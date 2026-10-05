//! Microphone capture for voice input: record, resample to 16 kHz mono, and
//! hand the backend a base64 WAV that llama-mtmd-cli can read.

const TARGET_RATE = 16000;

export class VoiceRecorder {
  private stream: MediaStream | null = null;
  private recorder: MediaRecorder | null = null;
  private chunks: Blob[] = [];

  get active(): boolean {
    return this.recorder !== null;
  }

  async start(): Promise<void> {
    if (this.recorder) return;
    this.stream = await navigator.mediaDevices.getUserMedia({
      audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
    });
    this.chunks = [];
    this.recorder = new MediaRecorder(this.stream);
    this.recorder.ondataavailable = (e) => {
      if (e.data.size > 0) this.chunks.push(e.data);
    };
    this.recorder.start();
  }

  /// Stop and return base64 WAV (16 kHz mono PCM16); empty when idle.
  async stop(): Promise<string> {
    const recorder = this.recorder;
    if (!recorder) return "";
    const blob = await new Promise<Blob>((resolve) => {
      recorder.onstop = () =>
        resolve(new Blob(this.chunks, { type: recorder.mimeType || "audio/webm" }));
      recorder.stop();
    });
    this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = null;
    this.recorder = null;
    this.chunks = [];
    const pcm = await decodeToPcm(blob);
    return bytesToBase64(encodeWav(pcm, TARGET_RATE));
  }
}

/// Decode the recorded container and resample to 16 kHz mono.
async function decodeToPcm(blob: Blob): Promise<Float32Array> {
  const data = await blob.arrayBuffer();
  const ctx = new AudioContext();
  const decoded = await ctx.decodeAudioData(data);
  await ctx.close();
  const length = Math.max(1, Math.ceil(decoded.duration * TARGET_RATE));
  const offline = new OfflineAudioContext(1, length, TARGET_RATE);
  const source = offline.createBufferSource();
  source.buffer = decoded;
  source.connect(offline.destination);
  source.start();
  const rendered = await offline.startRendering();
  return rendered.getChannelData(0);
}

/// PCM16 mono WAV container (44-byte header + samples).
export function encodeWav(samples: Float32Array, sampleRate: number): Uint8Array {
  const bytes = new Uint8Array(44 + samples.length * 2);
  const view = new DataView(bytes.buffer);
  const writeStr = (offset: number, s: string) => {
    for (let i = 0; i < s.length; i++) bytes[offset + i] = s.charCodeAt(i);
  };
  writeStr(0, "RIFF");
  view.setUint32(4, 36 + samples.length * 2, true);
  writeStr(8, "WAVE");
  writeStr(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // mono
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true); // byte rate
  view.setUint16(32, 2, true); // block align
  view.setUint16(34, 16, true); // bits per sample
  writeStr(36, "data");
  view.setUint32(40, samples.length * 2, true);
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(44 + i * 2, s < 0 ? s * 0x8000 : s * 0x7fff, true);
  }
  return bytes;
}

/// Chunked so a long recording cannot blow the argument limit.
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}
