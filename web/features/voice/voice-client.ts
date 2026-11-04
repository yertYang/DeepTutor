import { apiFetch } from '@/lib/api'
import { scopedUrl } from '@/lib/workspace-scope'
import type Recorder from 'opus-recorder'

export type VoiceStatus = 'idle' | 'permission' | 'connecting' | 'ready' | 'stopped' | 'error'
type Call = { id: string; ticket: string; websocket_path: string }
type Callbacks = {
  status: (status: VoiceStatus) => void; error: (message: string) => void;
  text: (text: string) => void; played: (seconds: number) => void;
}

/** One explicit call per instance. No reconnect or audio replay after failure. */
export class VoiceClient {
  private stopped = false
  private abort = new AbortController()
  private context?: AudioContext
  private stream?: MediaStream
  private recorder?: Recorder
  private decoder?: Worker
  private playback?: AudioWorkletNode
  private volume?: GainNode
  private socket?: WebSocket
  private call?: Call
  private timer?: ReturnType<typeof setTimeout>
  private microphoneMuted = false
  constructor(private scope: string, private callbacks: Callbacks) {}

  private fail(message: string) {
    if (this.stopped) return
    this.callbacks.error(message)
    this.stop('error')
  }

  async start(revision: number) {
    try {
      if (!window.isSecureContext || !navigator.mediaDevices?.getUserMedia || !window.AudioContext)
        throw new Error('Microphone access requires a secure browser context.')
      this.callbacks.status('permission')
      this.context = new AudioContext({ latencyHint: 'interactive' })
      await this.context.resume()
      this.stream = await navigator.mediaDevices.getUserMedia({ audio: {
        channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true,
      }, video: false })
      if (this.stopped) { this.stream.getTracks().forEach(track => track.stop()); return }
      this.callbacks.status('connecting')
      const context = this.context
      const source = context.createMediaStreamSource(this.stream)
      const { default: RecorderClass } = await import('opus-recorder')
      if (this.stopped) return
      this.recorder = new RecorderClass({
        sourceNode: source, encoderPath: '/voice-codecs/encoderWorker.min.js',
        encoderSampleRate: 24000, encoderFrameSize: 20, maxFramesPerPage: 2,
        numberOfChannels: 1, bufferLength: Math.round(960 * context.sampleRate / 24000),
        streamPages: true, monitorGain: 0, encoderComplexity: 0, encoderApplication: 2049,
      })
      const recorder = this.recorder
      void recorder.initialize.then(() => {
        if (this.stopped) recorder.close()
      }).catch(() => this.fail('Could not start microphone audio encoding.'))
      await context.audioWorklet.addModule('/voice/playback.js')
      if (this.stopped) return
      this.playback = new AudioWorkletNode(context, 'voice-playback', { numberOfInputs: 0, numberOfOutputs: 1, outputChannelCount: [1] })
      this.volume = context.createGain()
      this.playback.connect(this.volume).connect(context.destination)
      this.playback.port.onmessage = ({ data }: MessageEvent<{ playedSeconds: number }>) => {
        if (!this.stopped) this.callbacks.played(data.playedSeconds)
      }
      const decoder = new Worker('/voice/decoder.js')
      this.decoder = decoder
      const decoderReady = new Promise<void>((resolve, reject) => {
        this.abort.signal.addEventListener('abort', () => reject(new DOMException('Voice call stopped', 'AbortError')), { once: true })
        this.timer = setTimeout(() => reject(new Error('Could not initialize the voice audio decoder.')), 10000)
        decoder.onmessage = ({ data }: MessageEvent<{ type?: string } | Float32Array[]>) => {
          if (this.stopped) return
          if (data && 'type' in data && data.type === 'ready') { clearTimeout(this.timer); resolve() }
          else if (Array.isArray(data) && data[0] instanceof Float32Array)
            this.playback?.port.postMessage({ frame: data[0] }, [data[0].buffer])
        }
        decoder.onerror = () => reject(new Error('Could not initialize the voice audio decoder.'))
      })
      decoder.postMessage({ command: 'init', bufferLength: Math.round(960 * context.sampleRate / 24000),
        decoderSampleRate: 24000, outputBufferSampleRate: context.sampleRate, resampleQuality: 3 })
      await decoderReady
      decoder.onerror = () => this.fail('Voice audio decoding failed.')
      if (this.stopped) return
      const response = await apiFetch(scopedUrl('/api/voice/sessions', this.scope), {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ expected_revision: revision }), signal: this.abort.signal,
      })
      const call = await response.json() as Call & { message?: string }
      if (!response.ok) throw new Error(call.message || 'Could not start the voice conversation.')
      this.call = call
      if (this.stopped) { this.cancel(); return }
      const url = new URL(scopedUrl(call.websocket_path, this.scope), window.location.href)
      url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
      url.searchParams.set('ticket', call.ticket)
      const socket = new WebSocket(url)
      this.socket = socket
      socket.binaryType = 'arraybuffer'
      let ready = false
      let textBytes = 0
      this.timer = setTimeout(() => this.fail('PersonaPlex did not become ready in time.'), 125000)
      this.recorder.ondataavailable = data => {
        if (this.stopped || !ready || socket.readyState !== WebSocket.OPEN) return
        if (socket.bufferedAmount > 128 * 1024) { this.fail('The voice connection is too slow. Start a new conversation.'); return }
        const packet = new Uint8Array(data.byteLength + 1)
        packet[0] = 1; packet.set(data, 1); socket.send(packet)
      }
      socket.onmessage = ({ data }: MessageEvent<ArrayBuffer | string>) => {
        if (this.stopped) return
        if (typeof data === 'string') {
          try { const value = JSON.parse(data) as { type?: string; message?: string }; if (value.type === 'error') this.fail(value.message || 'The voice service returned an error.') }
          catch { this.fail('The voice service returned an invalid message.') }
          return
        }
        const packet = new Uint8Array(data)
        if (packet[0] === 0 && packet.length === 1 && !ready) {
          ready = true; clearTimeout(this.timer)
          this.recorder?.start().then(() => {
            if (this.stopped) recorder.close()
            else { recorder.setRecordingGain(this.microphoneMuted ? 0 : 1); this.callbacks.status('ready') }
          }).catch(() => this.fail('Could not start microphone audio encoding.'))
        } else if (ready && packet[0] === 1) {
          const pages = packet.slice(1)
          decoder.postMessage({ command: 'decode', pages }, [pages.buffer])
        } else if (ready && packet[0] === 2) {
          textBytes += packet.length
          if (textBytes > 256 * 1024) { this.fail('Voice transcript exceeded its limit.'); return }
          try { this.callbacks.text(new TextDecoder('utf-8', { fatal: true }).decode(packet.subarray(1))) }
          catch { this.fail('The voice service returned an invalid message.') }
        } else this.fail('The voice service returned an invalid message.')
      }
      socket.onerror = () => this.fail('The voice connection failed. Check the service and try again.')
      socket.onclose = () => { if (!this.stopped) this.fail('PersonaPlex ended the conversation.') }
    } catch (error) {
      if (!this.stopped) this.fail(error instanceof Error ? error.message : 'Could not start the voice conversation.')
    }
  }

  muteMicrophone(muted: boolean) {
    this.microphoneMuted = muted
    this.recorder?.setRecordingGain(muted ? 0 : 1)
    this.stream?.getAudioTracks().forEach(track => { track.enabled = !muted })
  }
  muteSpeaker(muted: boolean) { if (this.volume) this.volume.gain.value = muted ? 0 : 1 }
  private cancel() {
    if (this.call) void apiFetch(scopedUrl(`/api/voice/sessions/${encodeURIComponent(this.call.id)}/cancel`, this.scope),
      { method: 'POST', keepalive: true }).catch(() => {})
  }
  stop(status: VoiceStatus = 'stopped') {
    if (this.stopped) return
    this.stopped = true
    clearTimeout(this.timer); this.abort.abort(); this.cancel()
    if (this.socket) { this.socket.onclose = null; this.socket.onerror = null; this.socket.onmessage = null; this.socket.close() }
    this.stream?.getTracks().forEach(track => track.stop())
    try { this.recorder?.close() } catch { /* Already closed after an encoder error. */ }
    this.decoder?.terminate(); this.playback?.disconnect(); this.volume?.disconnect()
    void this.context?.close().catch(() => {})
    this.callbacks.status(status)
  }
}
