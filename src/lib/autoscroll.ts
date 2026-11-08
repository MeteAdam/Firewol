/** How close to a list's top or bottom edge the pointer must be to scroll it. */
const EDGE = 40;

/** Scrolls `list` when `y` is near an edge, faster the further in; returns whether it moved. */
export function edgeScroll(list: HTMLElement, y: number) {
  const box = list.getBoundingClientRect();
  const speed = y < box.top + EDGE ? -(box.top + EDGE - y) : y > box.bottom - EDGE ? y - (box.bottom - EDGE) : 0;
  if (!speed) return false;
  const before = list.scrollTop;
  list.scrollTop += Math.round(speed / 2);
  return list.scrollTop !== before;
}
