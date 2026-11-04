declare module 'opus-recorder' {
  export default class Recorder {
    constructor(options: {
      sourceNode: MediaStreamAudioSourceNode; encoderPath: string; encoderSampleRate: number;
      encoderFrameSize: number; maxFramesPerPage: number; numberOfChannels: number;
      bufferLength: number; streamPages: boolean; monitorGain: number; encoderComplexity: number;
      encoderApplication: number;
    })
    ondataavailable: (data: Uint8Array) => void
    initialize: Promise<void>
    start(): Promise<void>
    close(): void
    setRecordingGain(value: number): void
  }
}
