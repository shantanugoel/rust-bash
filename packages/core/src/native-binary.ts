/**
 * Resolve the packaged native addon filename for the current Node runtime.
 *
 * The main npm package bundles native binaries for the supported Linux/macOS
 * targets and the loader selects the matching file at runtime.
 */

export interface NativeRuntimeInfo {
  platform: NodeJS.Platform;
  arch: string;
}

const SUPPORTED_NATIVE_BINARIES: Record<string, string> = {
  'darwin:arm64': '../native/rust-bash-native.darwin-arm64.node',
  'darwin:x64': '../native/rust-bash-native.darwin-x64.node',
  'linux:arm64': '../native/rust-bash-native.linux-arm64-gnu.node',
  'linux:x64': '../native/rust-bash-native.linux-x64-gnu.node',
};

export function getNativeBinaryCandidates(runtime: NativeRuntimeInfo): string[] {
  const candidate = SUPPORTED_NATIVE_BINARIES[`${runtime.platform}:${runtime.arch}`];
  return candidate ? [candidate] : [];
}

export function getCurrentNativeBinaryCandidates(): string[] {
  return getNativeBinaryCandidates({
    platform: process.platform,
    arch: process.arch,
  });
}
