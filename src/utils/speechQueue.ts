//! Streams a reply into llama-tts while it is still generating: finished
//! sentences are synthesized in the background, so narration starts as soon
//! as the reply ends instead of after a fresh model load.

import { commands } from "../bindings";
import { call } from "./ipc";

/// Cut point for the next synthesis call. Every call spawns llama-tts, which
/// loads the model again, so chunks are large: the last completed sentence
/// once ~160 chars accumulated, or a clause boundary for run-on sentences.
function lastSentenceEnd(text: string): number {
  const sentence = /[.!?…]["')\]]*\s|\n/g;
  let cut = 0;
  for (const m of text.matchAll(sentence)) {
    const end = (m.index ?? 0) + m[0].length;
    if (end >= 160) cut = end;
  }
  if (cut > 0) return cut;
  if (text.length < 220) return 0;
  const clause = /[,;:]\s/g;
  for (const m of text.matchAll(clause)) {
    const end = (m.index ?? 0) + m[0].length;
    if (end >= 160) cut = end;
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
  errorReported = false;

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
  private playing = false;
  /// Playback state for the UI: the glow keeps running while audio plays.
  onPlayback: ((playing: boolean) => void) | null = null;
  /// One narration failure report per reply (never spam per chunk).
  onError: ((message: string) => void) | null = null;

  private setPlaying(playing: boolean): void {
    if (this.playing === playing) return;
    this.playing = playing;
    this.onPlayback?.(playing);
  }

  /// Start a new reply; anything queued or playing is dropped.
  reset(): void {
    stopAudio();
    this.setPlaying(false);
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
    this.setPlaying(false);
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
    // Narration counts as active from the first synthesis: the model load
    // happens before any audio, and the UI glow must not blink off in the gap.
    this.setPlaying(true);
    if (reply.pending === 0) reply.synthDone = true;
    while (this.reply === reply) {
      if (reply.audio.length > 0) {
        this.setPlaying(true);
        await playAudio(reply.audio.shift() as string);
      } else if (reply.synthDone) {
        break;
      } else {
        await reply.wait();
      }
    }
    this.setPlaying(false);
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
      .catch((e) => {
        // Narration failures used to vanish silently; report once per reply.
        if (this.reply === reply && !reply.errorReported) {
          reply.errorReported = true;
          this.onError?.(String(e));
        }
      })
      .then(() => {
        reply.pending -= 1;
        if (reply.pending === 0) reply.synthDone = true;
        reply.flush();
      });
  }
}
