import { createAudioPlayer, type AudioPlayer } from "expo-audio";
import { diagnostics } from "../../util/diagnostics";
import type { SoundPort } from "./presenter";

// Bundled chimes (self-generated, <1s). Players are created lazily on first
// play so importing this module never touches the audio session — important
// because this module loads at app start, before any alert can fire.
// Playback honours the silent switch / audio focus defaults: an alert should
// never blast over a silenced phone.

let completionPlayer: AudioPlayer | null = null;
let interruptionPlayer: AudioPlayer | null = null;

function play(player: AudioPlayer | null, create: () => AudioPlayer): AudioPlayer | null {
  try {
    const p = player ?? create();
    // Restart from the top so back-to-back turns re-chime instead of no-oping
    // at the end of the previous playback.
    void p
      .seekTo(0)
      .then(() => p.play())
      .catch((e) => {
        diagnostics.log(
          "alerts",
          `chime playback failed: ${e instanceof Error ? e.message : String(e)}`,
        );
      });
    return p;
  } catch (e) {
    diagnostics.log(
      "alerts",
      `chime player create failed: ${e instanceof Error ? e.message : String(e)}`,
    );
    return player;
  }
}

export const expoSound: SoundPort = {
  playCompletion: () => {
    completionPlayer = play(completionPlayer, () =>
      createAudioPlayer(require("../../../assets/turn_complete.wav")),
    );
  },
  playInterruption: () => {
    interruptionPlayer = play(interruptionPlayer, () =>
      createAudioPlayer(require("../../../assets/turn_interrupted.wav")),
    );
  },
};
