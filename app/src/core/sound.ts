// SoundEngine — Zuko's sounds, synthesized at preload. There are no audio
// files: every sound is a small recipe of oscillator tones and filtered noise
// bursts, rendered sample by sample into an AudioBuffer once at startup.
// Default volume 0.12, slider range 0–0.2, and several sounds may overlap.

export const SOUND_NAMES = [
  "peek", "open", "close", "hover", "blip", "slap", "annoyed", "dizzy", "greet",
  "work", "finish", "error", "approval", "question", "approve", "gulp", "tick",
  "send", "love", "pop", "proud", "wink", "yawn", "attach", "think", "search",
  "rate", "sleep", "fire", "fireball", "burst",
] as const;

export type SoundName = (typeof SOUND_NAMES)[number];

// ── Synth ─────────────────────────────────────────────────────────────────────

type Wave = "sine" | "tri" | "square" | "saw";

interface Tone {
  /** Start and end frequency (Hz); the glide is exponential. */
  f: number;
  f2?: number;
  /** Start time and duration, seconds. */
  at?: number;
  dur: number;
  wave?: Wave;
  gain?: number;
  /** Attack (s) and decay curve exponent (higher = more percussive). */
  attack?: number;
  curve?: number;
  /** Vibrato: rate (Hz) and depth (fraction of the frequency). */
  vib?: readonly [number, number];
  /** Level of the octave partial, for a glassy, bell-like tone. */
  bell?: number;
}

interface Noise {
  at?: number;
  dur: number;
  gain?: number;
  /** Band-pass centre (Hz), swept to `band2`, with quality `q`. */
  band: number;
  band2?: number;
  q?: number;
  attack?: number;
  curve?: number;
}

interface Recipe {
  tones?: Tone[];
  noise?: Noise[];
  /** Peak level after normalisation, 0…1. */
  peak?: number;
}

const note = (n: number) => 440 * Math.pow(2, (n - 69) / 12); // MIDI → Hz
const C5 = note(72), D5 = note(74), E5 = note(76), G5 = note(79), A5 = note(81), B5 = note(83);
const C6 = note(84), D6 = note(86), E6 = note(88), G6 = note(91);
const A4 = note(69), B4 = note(71), G4 = note(67), C4 = note(60);

/**
 * Fire crackle: a scatter of tiny band-passed noise pops, seeded so it is the
 * same every launch.
 */
function crackle(seed: number, n: number, from: number, span: number, gain: number): Noise[] {
  const rnd = mulberry32(seed);
  return Array.from({ length: n }, () => ({
    at: from + rnd() * span,
    dur: 0.006 + rnd() * 0.012,
    band: 1800 + rnd() * 4200,
    q: 2.5,
    gain: gain * (0.5 + rnd() * 0.5),
    curve: 3,
  }));
}

/**
 * The 31 recipes. Short, soft and techy-friendly: sine and triangle tones with
 * smooth envelopes, a little filtered noise for clicks, whooshes and fire.
 */
const RECIPES: Record<SoundName, Recipe> = {
  // Island chrome
  peek: { tones: [{ f: 520, f2: 780, dur: 0.09, bell: 0.15 }, { f: 1040, at: 0.05, dur: 0.07, gain: 0.3 }], peak: 0.6 },
  open: {
    tones: [{ f: 360, f2: 760, dur: 0.16, wave: "tri", curve: 1.6 }],
    noise: [{ dur: 0.14, band: 900, band2: 3200, q: 1.2, gain: 0.25 }],
    peak: 0.6,
  },
  close: {
    tones: [{ f: 720, f2: 340, dur: 0.15, wave: "tri", curve: 1.6 }],
    noise: [{ dur: 0.12, band: 2800, band2: 800, q: 1.2, gain: 0.22 }],
    peak: 0.55,
  },
  hover: { tones: [{ f: 1900, dur: 0.028, curve: 3 }], noise: [{ dur: 0.012, band: 5000, q: 2, gain: 0.3 }], peak: 0.35 },
  tick: { tones: [{ f: 2600, dur: 0.02, curve: 3 }], noise: [{ dur: 0.008, band: 7000, q: 2, gain: 0.4 }], peak: 0.32 },
  blip: { tones: [{ f: 640, f2: 470, dur: 0.08, wave: "tri", curve: 2 }], peak: 0.55 },
  attach: {
    tones: [{ f: 1400, dur: 0.03, curve: 3 }, { f: 1850, at: 0.055, dur: 0.035, curve: 3 }],
    noise: [{ dur: 0.01, band: 4500, q: 2, gain: 0.3 }, { at: 0.055, dur: 0.01, band: 5500, q: 2, gain: 0.3 }],
    peak: 0.45,
  },
  send: {
    tones: [{ f: 520, f2: 1240, dur: 0.18, curve: 1.8, bell: 0.1 }],
    noise: [{ dur: 0.2, band: 700, band2: 4200, q: 1.4, gain: 0.35, attack: 0.04 }],
    peak: 0.55,
  },
  pop: { tones: [{ f: 280, f2: 980, dur: 0.06, curve: 2.4 }], peak: 0.55 },

  // Agent states
  work: { tones: [{ f: C5, dur: 0.09, bell: 0.2 }, { f: E5, at: 0.07, dur: 0.14, bell: 0.2 }], peak: 0.5 },
  think: {
    tones: [
      { f: A4, dur: 0.09, wave: "tri", gain: 0.8 },
      { f: B4, at: 0.1, dur: 0.09, wave: "tri", gain: 0.8 },
      { f: A4, at: 0.2, dur: 0.14, wave: "tri", gain: 0.8 },
    ],
    peak: 0.4,
  },
  search: {
    tones: [
      { f: 1250, dur: 0.42, curve: 3.2, bell: 0.08 },
      { f: 1250, at: 0.2, dur: 0.3, curve: 3.2, gain: 0.3 },
    ],
    peak: 0.45,
  },
  approval: {
    // Two-tone rising chime: attention, but friendly.
    tones: [
      { f: E5, dur: 0.32, curve: 2.6, bell: 0.35 },
      { f: B5, at: 0.13, dur: 0.42, curve: 2.6, bell: 0.35 },
      { f: E6, at: 0.13, dur: 0.3, curve: 3, gain: 0.12 },
    ],
    peak: 0.7,
  },
  question: { tones: [{ f: C5, f2: G5, dur: 0.17, wave: "tri", curve: 1.5 }, { f: A5, at: 0.16, dur: 0.12, bell: 0.2 }], peak: 0.55 },
  approve: { tones: [{ f: G5, dur: 0.07, bell: 0.25 }, { f: D6, at: 0.06, dur: 0.16, bell: 0.25, curve: 2.4 }], peak: 0.6 },
  error: {
    // Low descending blip, twice.
    tones: [
      { f: 330, f2: 220, dur: 0.14, wave: "square", gain: 0.6, curve: 1.6 },
      { f: 247, f2: 156, at: 0.15, dur: 0.2, wave: "square", gain: 0.6, curve: 1.8 },
    ],
    peak: 0.55,
  },
  finish: {
    // Bright arpeggio up to the octave.
    tones: [
      { f: C5, dur: 0.12, bell: 0.3 },
      { f: E5, at: 0.065, dur: 0.12, bell: 0.3 },
      { f: G5, at: 0.13, dur: 0.12, bell: 0.3 },
      { f: C6, at: 0.195, dur: 0.45, bell: 0.35, curve: 2.6 },
      { f: G6, at: 0.195, dur: 0.3, gain: 0.1, curve: 3 },
    ],
    peak: 0.65,
  },
  rate: {
    tones: [
      { f: B4, f2: 466, dur: 0.2, wave: "tri" },
      { f: G4, f2: 370, at: 0.18, dur: 0.32, wave: "tri", curve: 1.6 },
    ],
    peak: 0.5,
  },
  sleep: { tones: [{ f: G4, f2: C4, dur: 0.55, wave: "tri", attack: 0.06, curve: 1.4, vib: [5, 0.01] }], peak: 0.4 },

  // Zuko's moods
  greet: {
    // A fiery whoosh spinning up, the landing thump, and a bright sparkle as
    // the eyes ignite.
    tones: [
      { f: 120, f2: 70, at: 0.5, dur: 0.16, curve: 2.2, gain: 0.8 },
      { f: E6, at: 0.72, dur: 0.32, bell: 0.3, curve: 2.8, gain: 0.35 },
      { f: G6, at: 0.8, dur: 0.36, curve: 3, gain: 0.25 },
    ],
    noise: [
      { dur: 0.55, band: 300, band2: 2200, q: 0.8, gain: 0.9, attack: 0.3, curve: 1.6 },
      { at: 0.48, dur: 0.12, band: 500, q: 0.8, gain: 0.5, curve: 3 },
      ...crackle(41, 9, 0.55, 0.45, 0.35),
    ],
    peak: 0.6,
  },
  fire: {
    // Flame catching: a soft rising whoosh with crackle.
    tones: [{ f: 90, f2: 160, dur: 0.3, wave: "tri", attack: 0.08, curve: 1.6, gain: 0.35 }],
    noise: [
      { dur: 0.34, band: 400, band2: 1600, q: 0.7, gain: 1, attack: 0.08, curve: 1.8 },
      ...crackle(7, 8, 0.04, 0.3, 0.5),
    ],
    peak: 0.5,
  },
  fireball: {
    // The launch: a fast whoosh sweeping up and away.
    tones: [{ f: 220, f2: 90, dur: 0.22, wave: "tri", curve: 1.8, gain: 0.4 }],
    noise: [
      { dur: 0.3, band: 700, band2: 3600, q: 1.1, gain: 1, attack: 0.02, curve: 1.5 },
      ...crackle(13, 5, 0.05, 0.2, 0.35),
    ],
    peak: 0.55,
  },
  burst: {
    // The impact: a low thump, a bright pop and a tail of crackle.
    tones: [
      { f: 150, f2: 55, dur: 0.22, curve: 2.4, gain: 1 },
      { f: 1400, f2: 700, dur: 0.06, curve: 2.6, gain: 0.25 },
    ],
    noise: [
      { dur: 0.3, band: 1400, band2: 400, q: 0.7, gain: 0.9, curve: 2.2 },
      ...crackle(29, 10, 0.05, 0.35, 0.45),
    ],
    peak: 0.6,
  },
  gulp: {
    // The pulse on a drop: a quick down-up sweep as the fists flare.
    tones: [{ f: 1300, f2: 700, dur: 0.07, curve: 1.4 }, { f: 700, f2: 1500, at: 0.07, dur: 0.1, curve: 2 }],
    noise: [{ dur: 0.18, band: 900, band2: 2400, q: 1, gain: 0.3 }, ...crackle(3, 4, 0.04, 0.12, 0.25)],
    peak: 0.5,
  },
  slap: {
    tones: [{ f: 190, f2: 85, dur: 0.12, curve: 2.4 }],
    noise: [{ dur: 0.05, band: 1100, q: 0.8, gain: 0.8, curve: 3 }],
    peak: 0.6,
  },
  annoyed: {
    tones: [
      { f: 340, f2: 300, dur: 0.08, wave: "square", gain: 0.5 },
      { f: 300, f2: 240, at: 0.1, dur: 0.12, wave: "square", gain: 0.5 },
    ],
    peak: 0.45,
  },
  dizzy: { tones: [{ f: 760, f2: 380, dur: 0.6, wave: "tri", vib: [11, 0.08], curve: 1.3 }], peak: 0.45 },
  love: {
    tones: [
      { f: E5, dur: 0.16, vib: [6, 0.012], bell: 0.2 },
      { f: A5, at: 0.13, dur: 0.32, vib: [6, 0.012], bell: 0.2, curve: 2.2 },
    ],
    peak: 0.55,
  },
  proud: { tones: [{ f: G5, dur: 0.1, wave: "tri" }, { f: C6, at: 0.1, dur: 0.32, wave: "tri", bell: 0.2, curve: 2.2 }], peak: 0.55 },
  wink: { tones: [{ f: C6, f2: E6, dur: 0.07, curve: 2 }, { f: G6, at: 0.08, dur: 0.06, curve: 3, gain: 0.5 }], peak: 0.45 },
  yawn: { tones: [{ f: D5, f2: 250, dur: 0.65, wave: "tri", attack: 0.12, curve: 1.3, vib: [4.5, 0.015] }], peak: 0.4 },
};


/** Small deterministic PRNG so every launch renders the same noise. */
function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function osc(wave: Wave, p: number): number {
  switch (wave) {
    case "sine":
      return Math.sin(p);
    case "tri":
      return (2 / Math.PI) * Math.asin(Math.sin(p));
    case "square":
      // A few odd harmonics: a soft, rounded square.
      return 0.9 * (Math.sin(p) + Math.sin(3 * p) / 3 + Math.sin(5 * p) / 5 + Math.sin(7 * p) / 7);
    case "saw":
      return 0.6 * (Math.sin(p) - Math.sin(2 * p) / 2 + Math.sin(3 * p) / 3 - Math.sin(4 * p) / 4);
  }
}

function envelope(tt: number, dur: number, attack: number, curve: number): number {
  if (tt < attack) return tt / attack;
  const k = 1 - (tt - attack) / Math.max(1e-4, dur - attack);
  return k <= 0 ? 0 : Math.pow(k, curve);
}

/** Renders one sound to mono samples at `sr` Hz. Pure; no Web Audio needed. */
export function synthSamples(name: SoundName, sr: number): Float32Array {
  const r = RECIPES[name];
  let end = 0;
  for (const t of r.tones ?? []) end = Math.max(end, (t.at ?? 0) + t.dur);
  for (const n of r.noise ?? []) end = Math.max(end, (n.at ?? 0) + n.dur);
  const out = new Float32Array(Math.ceil((end + 0.01) * sr));

  for (const t of r.tones ?? []) {
    const start = Math.round((t.at ?? 0) * sr);
    const len = Math.round(t.dur * sr);
    const f2 = t.f2 ?? t.f;
    const wave = t.wave ?? "sine";
    const gain = t.gain ?? 1;
    const attack = t.attack ?? 0.004;
    const curve = t.curve ?? 2;
    let phase = 0;
    for (let i = 0; i < len && start + i < out.length; i++) {
      const tt = i / sr;
      let f = t.f * Math.pow(f2 / t.f, i / len);
      if (t.vib) f *= 1 + t.vib[1] * Math.sin(2 * Math.PI * t.vib[0] * tt);
      phase += (2 * Math.PI * f) / sr;
      let v = osc(wave, phase);
      if (t.bell) v += t.bell * Math.sin(2 * phase);
      out[start + i] += gain * envelope(tt, t.dur, attack, curve) * v;
    }
  }

  const rnd = mulberry32(name.length * 7919 + name.charCodeAt(0));
  for (const n of r.noise ?? []) {
    const start = Math.round((n.at ?? 0) * sr);
    const len = Math.round(n.dur * sr);
    const b2 = n.band2 ?? n.band;
    const q = n.q ?? 1;
    const gain = n.gain ?? 1;
    const attack = n.attack ?? 0.002;
    const curve = n.curve ?? 2;
    let x1 = 0, x2 = 0, y1 = 0, y2 = 0;
    for (let i = 0; i < len && start + i < out.length; i++) {
      // RBJ band-pass (0 dB peak), its centre swept exponentially.
      const fc = Math.min(sr * 0.45, n.band * Math.pow(b2 / n.band, i / len));
      const w0 = (2 * Math.PI * fc) / sr;
      const alpha = Math.sin(w0) / (2 * q);
      const a0 = 1 + alpha;
      const x0 = rnd() * 2 - 1;
      const y0 = (alpha * x0 - alpha * x2 + 2 * Math.cos(w0) * y1 - (1 - alpha) * y2) / a0;
      x2 = x1; x1 = x0; y2 = y1; y1 = y0;
      out[start + i] += gain * envelope(i / sr, n.dur, attack, curve) * y0;
    }
  }

  let peak = 0;
  for (const v of out) peak = Math.max(peak, Math.abs(v));
  if (peak > 0) {
    const k = (r.peak ?? 0.6) / peak;
    for (let i = 0; i < out.length; i++) out[i] *= k;
  }
  return out;
}

// ── Engine ────────────────────────────────────────────────────────────────────

class SoundEngine {
  enabled = true;
  volume = 0.12;

  private ctx: AudioContext | null = null;
  private master: GainNode | null = null;
  private buffers = new Map<string, AudioBuffer>();
  private loading: Promise<void> | null = null;
  private idleTimer: number | null = null;

  /** Creates the context and synthesizes every sound. Safe to call more than once. */
  preload(): Promise<void> {
    if (this.loading) return this.loading;
    this.loading = (async () => {
      const Ctor = window.AudioContext ?? (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
      if (!Ctor) return;
      const ctx = new Ctor();
      this.ctx = ctx;
      const master = ctx.createGain();
      master.gain.value = this.volume;
      master.connect(ctx.destination);
      this.master = master;
      for (const name of SOUND_NAMES) {
        try {
          const samples = synthSamples(name, ctx.sampleRate);
          const buf = ctx.createBuffer(1, samples.length, ctx.sampleRate);
          buf.getChannelData(0).set(samples);
          this.buffers.set(name, buf);
        } catch {
          /* a missing sound must never break the island */
        }
        // Yield between sounds so startup never blocks a frame for long.
        await new Promise<void>((r) => window.setTimeout(r, 0));
      }
    })();
    return this.loading;
  }

  /** WebView2 can hand us a suspended context; call after any user input. */
  resume() {
    if (this.idleTimer != null) {
      window.clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
    void this.ctx?.resume();
  }

  /**
   * Called when the island goes quiet. A running AudioContext keeps an audio
   * thread and its render quantum alive even with nothing playing, which shows
   * up as a steady trickle of CPU on a machine that is supposed to be idle.
   *
   * The delay covers the tail of whatever just played — suspending mid-sound
   * would clip it — and `play()` resumes the context on its own.
   */
  idle() {
    if (!this.ctx || this.ctx.state !== "running" || this.idleTimer != null) return;
    this.idleTimer = window.setTimeout(() => {
      this.idleTimer = null;
      void this.ctx?.suspend();
    }, 1500);
  }

  setVolume(v: number) {
    this.volume = Math.max(0, Math.min(0.2, v));
    if (this.master) this.master.gain.value = this.volume;
  }

  setEnabled(on: boolean) {
    this.enabled = on;
  }

  play(name: SoundName | string) {
    if (!this.enabled) return;
    const ctx = this.ctx;
    const master = this.master;
    const buf = this.buffers.get(name);
    if (!ctx || !master || !buf) return;
    if (this.idleTimer != null) {
      window.clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
    if (ctx.state === "suspended") void ctx.resume();
    const src = ctx.createBufferSource();
    src.buffer = buf;
    src.connect(master);
    src.start();
  }
}

export const Sound = new SoundEngine();
