// Lorevia for MacOS with Intel Silicon. Builds on an Intel Mac, or on
// Apple Silicon through Rosetta: every build tool runs from the x64 toolchain.
export default {
  id: 'macos-x64', platform: 'darwin', arch: 'x64',
  node: { version: '22.23.4', file: 'node-v22.23.4-darwin-x64.tar.gz', sha256: 'efcc2b0db6f94bf53eddb038fe2b370277a042bd29e4f19b8f8e23598795b819' },
  artifact: 'Lorevia-MacOS-Intel',
};
