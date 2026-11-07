import { api } from "./api";
import type { VolumeState } from "./contract";

/** NTFS volume indexes; shared by every window and announced through `volume-state`. */
class Volumes {
  states = $state.raw<Record<string, VolumeState>>({});

  update(state: VolumeState) {
    const next = { ...this.states };
    if (state.running) next[state.drive] = state;
    else delete next[state.drive];
    this.states = next;
  }

  async load() {
    for (const state of await api.volumeStates()) this.update(state);
  }
}

export const volumes = new Volumes();
