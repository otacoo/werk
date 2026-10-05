//! Streams a reply into llama-tts while it is still generating: finished
//! sentences are synthesized in the background, so narration starts as soon
//! as the reply ends instead of after a fresh model load.

import { commands } from "../bindings";
import { call } from "./ipc";

/// Index just past the last sentence boundary, or 0 when none is long enough
/// to be worth its own synthesis call.
function lastSentenceEnd(text: string): number {
  const re = /[.!?…]["')\]]*\s|\n/g;
  let cut = 0;
  for (const m of text.matchAll(re)) {
    const end = (m.index ?? 0) + m[0].length;
    if (end >= 24) cut = end;
  }
  return cut;
}

/// One reply's narration state; replaced wholesale on reset so stale
/// synthesis results can tell they are no longer wanted.
class Reply {
  audio: string[] = [];
  waiters: (() => void)[] = [];
  pending = 0;
  synthDone = false;
  aborted = false;
  queued = 0;
  epoch = 0;

  flush(): void {
    const waiters = this.waiters;
    this.waiters = [];
    for (const resume of waiters) resume();
  }

  wait(): Promise<void> {
    return new Promise((resolve) => this.waiters.push(resolve));
  }
}

let currentAudio: HTMLAudioElement | null = null;

function stopAudio(): void {
  if (currentAudio) {
    currentAudio.pause();
    currentAudio = null;
  }
}

function playAudio(audio: string): Promise<void> {
  return new Promise((resolve) => {
    const el = new Audio(`data:audio/wav;base64,${audio}`);
    currentAudio = el;
    const done = () => {
      if (currentAudio === el) currentAudio = null;
      resolve();
    };
    el.onended = done;
    el.onerror = done;
    el.play().catch(done);
  });
}

export class SpeechQueue {
  private reply = new Reply();
  private chain: Promise<void> = Promise.resolve();

  /// Start a new reply; anything queued or playing is dropped.
  reset(): void {
    stopAudio();
    this.reply = new Reply();
    this.chain = Promise.resolve();
  }

  /// Feed the growing reply; complete sentences are synthesized in order.
  push(text: string): void {
    const reply = this.reply;
    if (reply.aborted) return;
    const tail = text.slice(reply.queued);
    const cut = lastSentenceEnd(tail);
    if (cut === 0) return;
    reply.queued += cut;
    this.enqueue(reply, tail.slice(0, cut));
  }

  /// The turn used tools: drop queued speech and wait for the final text.
  abort(): void {
    stopAudio();
    this.reply.aborted = true;
    this.reply.epoch += 1;
    this.reply.audio = [];
  }

  /// Synthesize what is left and play the chunks in order, as they arrive.
  async finish(finalText: string): Promise<void> {
    const reply = this.reply;
    const tail = reply.aborted ? finalText : finalText.slice(reply.queued);
    if (tail.trim()) {
      reply.aborted = false;
      reply.queued = finalText.length;
      this.enqueue(reply, tail);
    }
    if (reply.pending === 0) reply.synthDone = true;
    while (this.reply === reply) {
      if (reply.audio.length > 0) {
        await playAudio(reply.audio.shift() as string);
      } else if (reply.synthDone) {
        break;
      } else {
        await reply.wait();
      }
    }
    if (this.reply === reply) this.reset();
  }

  private enqueue(reply: Reply, chunk: string): void {
    const epoch = reply.epoch;
    reply.pending += 1;
    this.chain = this.chain
      .then(() => call(commands.assistantTtsSpeak(chunk)))
      .then((r) => {
        if (this.reply === reply && epoch === reply.epoch && !reply.aborted) {
          reply.audio.push(r.audio);
        }
      })
      .catch(() => {})
      .then(() => {
        reply.pending -= 1;
        if (reply.pending === 0) reply.synthDone = true;
        reply.flush();
      });
  }
}
