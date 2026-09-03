/**
 * The `napi:audio` capability: playback of local audio files through the
 * `miniaudio_node` npm package (native rodio backend).
 *
 * The native library is loaded on the HOST, never inside the interpreter
 * (see `native-loader.ts` for why). Each VM gets its own `AudioPlayer`, so
 * unloading the plugin drops playback state with the VM. Every file path the
 * guest passes is resolved through the plugin's `FsPermissionChecker` first:
 * `loadFile("/etc/passwd")` is a permission error, not a native open.
 *
 * Types are resolved from the installed package (`import type` below), never
 * hand-copied: if `miniaudio_node` renames a method, `tsc` fails here instead
 * of shipping a silently stale bridge. `AudioPlayerLike` is the guest-visible
 * subset — deliberately playback-scoped. `AudioDecoder` / `AudioRecorder` /
 * `AudioPassthrough` stay host-side until a plugin demonstrates it needs
 * them; each one is a new native sink and gets its own review.
 *
 * Guest API (`import ... from "napi:audio"`):
 *
 *   getDevices()        device list (no paths involved, always safe)
 *   loadFile(path)      guest path, checked against `fs.read` permission
 *   loadBuffer(samples) number array, length-capped
 *   loadBase64(data)    base64 string, byte-capped
 *   play() / pause() / stop()
 *   setVolume(v)        0..1, out-of-range is a RangeError
 *   getVolume() / isPlaying() / getState()
 *   getDuration() / getCurrentTime() / getCurrentFile() / seekTo(seconds)
 */

import { createRequire } from "node:module";

import type { AudioPlayer } from "miniaudio_node";

import { PluginLoadError } from "../core/errors";
import { installNativeModule } from "../native/native-loader";
import {
  applyCapabilityOptions,
  type CapabilityDefinition,
  type CapabilityOptionsSchema,
} from "./capability-registry";
import { definePermissionBinding } from "../core/manifest";

definePermissionBinding("audio", {});

export const AUDIO_MODULE_NAME = "napi:audio";

/** Default ceiling on a base64/buffer payload, in bytes. */
export const DEFAULT_MAX_AUDIO_BYTES = 8 * 1024 * 1024;

/**
 * Guest-visible subset of the installed `AudioPlayer`. A `Pick`, not a
 * copy: drift against `miniaudio_node` is a compile error, not a runtime
 * surprise.
 */
export type AudioPlayerLike = Pick<
  AudioPlayer,
  | "getDevices"
  | "loadFile"
  | "loadBuffer"
  | "loadBase64"
  | "play"
  | "pause"
  | "stop"
  | "setVolume"
  | "getVolume"
  | "isPlaying"
  | "getState"
  | "getDuration"
  | "getCurrentTime"
  | "getCurrentFile"
  | "seekTo"
>;

/**
 * Host-side grant options (`policy.capabilities.audio`). The player factory
 * lives here — host code — so tests can inject a fake without the native
 * library and production lazily `require`s it.
 */
export interface AudioPolicyOptions {
  maxAudioBytes?: number;
  createPlayer?: () => AudioPlayerLike;
}

function defaultCreatePlayer(): AudioPlayerLike {
  let AudioPlayerCtor: new () => AudioPlayer;
  try {
    const require = createRequire(import.meta.url);
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    AudioPlayerCtor = require("miniaudio_node").AudioPlayer;
  } catch {
    throw new PluginLoadError(
      'napi:audio needs the "miniaudio_node" package: npm install miniaudio_node',
    );
  }
  if (typeof AudioPlayerCtor !== "function") {
    throw new PluginLoadError('napi:audio: "miniaudio_node" did not export AudioPlayer');
  }
  return new AudioPlayerCtor();
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

/** Exposure policy: closed allowlist, path sink declared for `loadFile`. */
export const AUDIO_DEFINITION = {
  moduleName: AUDIO_MODULE_NAME,
  methods: {
    getDevices: {},
    loadFile: { pathArgs: [0] },
    loadBuffer: {
      validate: ([samples]: unknown[]) => {
        if (!Array.isArray(samples)) {
          throw new TypeError("loadBuffer(samples): samples must be an array");
        }
      },
    },
    loadBase64: {},
    play: {},
    pause: {},
    stop: {},
    setVolume: {
      validate: ([volume]: unknown[]) => {
        if (!isFiniteNumber(volume) || volume < 0 || volume > 1) {
          throw new RangeError("setVolume(v): v must be a number in 0..1");
        }
      },
    },
    getVolume: {},
    isPlaying: {},
    getState: {},
    getDuration: {},
    getCurrentTime: {},
    getCurrentFile: {},
    seekTo: {
      validate: ([position]: unknown[]) => {
        if (!isFiniteNumber(position) || position < 0) {
          throw new RangeError("seekTo(s): s must be a non-negative number");
        }
      },
    },
  },
};

const AUDIO_SCHEMA: CapabilityOptionsSchema = {
  maxAudioBytes: { type: "number", default: DEFAULT_MAX_AUDIO_BYTES, min: 1, integer: true },
};

/**
 * Registry entry: request with `capabilities: { "audio": true }` (or
 * `{ "audio": { "maxAudioBytes": n } }`). One player per VM; the teardown
 * the host runs on unload drops it with the VM.
 */
export const AUDIO_CAPABILITY: CapabilityDefinition = {
  name: "audio",
  schema: AUDIO_SCHEMA,
  install: ({ vm, checker, options, grant }) => {
    if (!checker) {
      throw new PluginLoadError('capability "audio" needs a filesystem checker');
    }
    const opts = applyCapabilityOptions("audio", AUDIO_SCHEMA, options);
    const policy = (grant !== null && typeof grant === "object" ? grant : {}) as AudioPolicyOptions;
    let maxAudioBytes = opts.maxAudioBytes as number;
    if (policy.maxAudioBytes !== undefined) {
      if (!isFiniteNumber(policy.maxAudioBytes) || policy.maxAudioBytes < 1) {
        throw new PluginLoadError('capability "audio": maxAudioBytes must be a number >= 1');
      }
      // The host grant wins over the manifest request: a plugin must never
      // widen its own payload ceiling.
      maxAudioBytes = Math.floor(policy.maxAudioBytes);
    }
    const player = (policy.createPlayer ?? defaultCreatePlayer)();
    const installed = installNativeModule(
      vm,
      { ...AUDIO_DEFINITION, maxStringBytes: maxAudioBytes },
      { target: player as unknown as Record<string, unknown>, checker },
    );
    return () => installed.uninstall(vm);
  },
};
