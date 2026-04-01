import { describe, expect, it } from 'vitest';
import { getNativeBinaryCandidates } from '../src/native-binary.js';

describe('native binary resolution', () => {
  it('resolves the linux x64 packaged addon', () => {
    expect(
      getNativeBinaryCandidates({ platform: 'linux', arch: 'x64' }),
    ).toEqual(['../native/rust-bash-native.linux-x64-gnu.node']);
  });

  it('resolves the linux arm64 packaged addon', () => {
    expect(
      getNativeBinaryCandidates({ platform: 'linux', arch: 'arm64' }),
    ).toEqual(['../native/rust-bash-native.linux-arm64-gnu.node']);
  });

  it('resolves the macOS x64 packaged addon', () => {
    expect(
      getNativeBinaryCandidates({ platform: 'darwin', arch: 'x64' }),
    ).toEqual(['../native/rust-bash-native.darwin-x64.node']);
  });

  it('resolves the macOS arm64 packaged addon', () => {
    expect(
      getNativeBinaryCandidates({ platform: 'darwin', arch: 'arm64' }),
    ).toEqual(['../native/rust-bash-native.darwin-arm64.node']);
  });

  it('returns no candidates for unsupported runtimes', () => {
    expect(
      getNativeBinaryCandidates({ platform: 'win32', arch: 'x64' }),
    ).toEqual([]);
  });
});
