import { cpSync, mkdirSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
export function copyVoiceAssets() {
  const target = path.join(root, 'public/voice-codecs')
  mkdirSync(target, { recursive: true })
  for (const name of ['encoderWorker.min.js', 'decoderWorker.min.js', 'decoderWorker.min.wasm'])
    cpSync(path.join(root, 'node_modules/opus-recorder/dist', name), path.join(target, name))
  cpSync(path.join(root, 'node_modules/opus-recorder/LICENSE.md'), path.join(target, 'LICENSE.txt'))
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) copyVoiceAssets()
