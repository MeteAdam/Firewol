/** What a pane opens to show the Recycle Bin; the core (`recycle_bin::RECYCLE_BIN`) uses the same text. */
export const RECYCLE_BIN = "shell:RecycleBinFolder";

const BIN_CLASS = "::{645ff040-5081-101b-9f08-00aa002f954e}";

/** Every way Windows names the Recycle Bin when it asks a folder program to open it. */
export function isRecycleBin(target: string) {
  const name = target.trim().replace(/\\+$/, "").toLowerCase();
  return name === RECYCLE_BIN.toLowerCase() || name === BIN_CLASS || name === `shell:${BIN_CLASS}`;
}
