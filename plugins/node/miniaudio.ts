/**
 * Explicit `miniaudio_node` player factory for operators who want to wire
 * `napi:audio` by hand — e.g. a grant built far from the `PluginHost`, or a
 * test double swapped per environment.
 *
 * On Node/Bun hosts you usually need none of this: the audio capability's
 * default player already loads `miniaudio_node` through the platform's
 * `requireNative`. This helper exists for the cases where the default is not
 * wanted.
 *
 * ```ts
 * import { createMiniaudioPlayer } from "napi-vm/plugins/node";
 *
 * const host = new PluginHost({
 *   platform: nodePlatform(),
 *   policy: { capabilities: { audio: { createPlayer: createMiniaudioPlayer } } },
 * });
 * ```
 */

import type { AudioPlayer } from "miniaudio_node";
import { createRequire } from "node:module";

import { PluginLoadError } from "../core/errors";
import type { AudioPlayerLike } from "../capabilities/audio-capability";

/**
 * Build an `AudioPlayer` from the installed `miniaudio_node` package,
 * resolved from the process working directory.
 */
export function createMiniaudioPlayer(
  load: (specifier: string) => unknown = (specifier: string) =>
    createRequire(`${process.cwd()}/package.json`)(specifier),
): AudioPlayerLike {
  let AudioPlayerCtor: new () => AudioPlayer;
  try {
    AudioPlayerCtor = (
      load("miniaudio_node") as { AudioPlayer: new () => AudioPlayer }
    ).AudioPlayer;
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
