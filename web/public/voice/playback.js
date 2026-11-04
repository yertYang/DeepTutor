class VoicePlayback extends AudioWorkletProcessor {
  constructor() {
    super();
    this.frames = [];
    this.offset = 0;
    this.samples = 0;
    this.started = false;
    this.played = 0;
    this.reportAt = 0;
    this.port.onmessage = ({ data }) => {
      if (data.type === 'reset') {
        this.frames = []; this.samples = 0; this.offset = 0; this.started = false;
      } else if (data.frame instanceof Float32Array && data.frame.length <= sampleRate) {
        this.frames.push(data.frame); this.samples += data.frame.length;
        // Keep latency bounded even after the page was briefly backgrounded.
        while (this.samples > sampleRate / 2 && this.frames.length > 1) {
          this.samples -= this.frames.shift().length - this.offset; this.offset = 0;
        }
      }
    };
  }
  process(_inputs, outputs) {
    const output = outputs[0][0];
    if (!this.started && this.samples >= sampleRate * 0.08) this.started = true;
    if (!this.started) return true;
    let pos = 0;
    while (pos < output.length && this.frames.length) {
      const first = this.frames[0];
      const count = Math.min(output.length - pos, first.length - this.offset);
      output.set(first.subarray(this.offset, this.offset + count), pos);
      this.offset += count; pos += count; this.samples -= count;
      if (this.offset === first.length) { this.frames.shift(); this.offset = 0; }
    }
    this.played += pos;
    if (!this.frames.length) this.started = false;
    if (this.played - this.reportAt >= sampleRate / 2) {
      this.reportAt = this.played;
      this.port.postMessage({ playedSeconds: this.played / sampleRate });
    }
    return true;
  }
}
registerProcessor('voice-playback', VoicePlayback);
