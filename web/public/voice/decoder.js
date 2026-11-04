// opus-recorder 8.0.5 exposes mainReady; wait for actual WASM initialization.
self.Module = { locateFile: name => `/voice-codecs/${name}` };
importScripts('/voice-codecs/decoderWorker.min.js');
self.DecoderWorker.mainReady.then(() => self.postMessage({ type: 'ready' }));
