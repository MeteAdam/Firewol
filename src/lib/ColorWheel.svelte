<script lang="ts" module>
  const SIZE = 168;
  const RADIUS = SIZE / 2 - 2;
  let palette: Uint8ClampedArray | undefined;
</script>

<script lang="ts">
  import { untrack } from "svelte";
  import { hexOf, hsvAt, hsvOf, pointOf, rgbOf, type Hsv, type Rgb } from "./color";

  let { color, label, onpick }: { color: Rgb; label: string; onpick: (rgb: Rgb) => void } = $props();
  let canvas = $state<HTMLCanvasElement>();
  let hsv = $state<Hsv>(untrack(() => hsvOf(color)));
  let dragging = false;
  const marker = $derived(pointOf(hsv, RADIUS));
  const brightness = $derived(hsv.value);

  // Follow a colour chosen elsewhere (another slot, a typed hex) without fighting the user's own pick.
  $effect(() => {
    const next = color;
    untrack(() => {
      if (!dragging && rgbOf(hsv).some((channel, index) => channel !== next[index])) hsv = hsvOf(next);
    });
  });

  // Full-brightness wheel computed once; each brightness only rescales it (28k pixels).
  $effect(() => {
    const context = canvas?.getContext("2d");
    if (!context) return;
    palette ??= (() => {
      const pixels = new Uint8ClampedArray(SIZE * SIZE * 4);
      for (let y = 0; y < SIZE; y++) {
        for (let x = 0; x < SIZE; x++) {
          const dx = x - SIZE / 2, dy = y - SIZE / 2;
          if (Math.hypot(dx, dy) > RADIUS) continue;
          const [r, g, b] = rgbOf(hsvAt(dx, dy, RADIUS, 1));
          pixels.set([r, g, b, 255], (y * SIZE + x) * 4);
        }
      }
      return pixels;
    })();
    const shaded = new Uint8ClampedArray(palette);
    for (let index = 0; index < shaded.length; index += 4) {
      shaded[index] *= brightness;
      shaded[index + 1] *= brightness;
      shaded[index + 2] *= brightness;
    }
    context.putImageData(new ImageData(shaded, SIZE, SIZE), 0, 0);
  });

  function set(next: Hsv) {
    hsv = next;
    onpick(rgbOf(next));
  }

  function pick(event: PointerEvent) {
    const box = canvas!.getBoundingClientRect();
    const scale = SIZE / box.width;
    set(hsvAt((event.clientX - box.left) * scale - SIZE / 2, (event.clientY - box.top) * scale - SIZE / 2, RADIUS, hsv.value));
  }

  function key(event: KeyboardEvent) {
    const step = { ArrowLeft: [-1 / 72, 0], ArrowRight: [1 / 72, 0], ArrowUp: [0, 0.05], ArrowDown: [0, -0.05] }[event.key];
    if (!step) return;
    event.preventDefault();
    set({ hue: (hsv.hue + step[0] + 1) % 1, saturation: Math.min(Math.max(hsv.saturation + step[1], 0), 1), value: hsv.value });
  }
</script>

<div class="wheel">
  <div class="disc">
    <canvas
      bind:this={canvas}
      width={SIZE}
      height={SIZE}
      tabindex="0"
      role="slider"
      aria-label={`${label} colour wheel`}
      aria-valuemin={0}
      aria-valuemax={359}
      aria-valuenow={Math.round(hsv.hue * 360)}
      aria-valuetext={hexOf(rgbOf(hsv))}
      onkeydown={key}
      onpointerdown={(event) => {
        dragging = true;
        try { canvas!.setPointerCapture(event.pointerId); } catch { /* A lost pointer still picks once. */ }
        pick(event);
      }}
      onpointermove={(event) => { if (dragging) pick(event); }}
      onpointerup={() => (dragging = false)}
      onpointercancel={() => (dragging = false)}
    ></canvas>
    <span class="marker" style:left={`${SIZE / 2 + marker.x}px`} style:top={`${SIZE / 2 + marker.y}px`} style:background={hexOf(rgbOf(hsv))}></span>
  </div>
  <label class="bright">
    <span class="muted">Brightness</span>
    <input type="range" min="0" max="100" value={Math.round(hsv.value * 100)} oninput={(event) => set({ ...hsv, value: Number(event.currentTarget.value) / 100 })} />
  </label>
</div>

<style>
  .wheel { display: flex; flex-direction: column; gap: 12px; width: 168px; flex: none; }
  .disc { position: relative; width: 168px; height: 168px; }
  canvas { display: block; border-radius: 50%; touch-action: none; }
  .marker { position: absolute; width: 14px; height: 14px; margin: -7px 0 0 -7px; border: 2px solid #ffffff; border-radius: 50%; box-shadow: 0 0 0 1px rgba(0, 0, 0, .45), 0 1px 3px rgba(0, 0, 0, .35); pointer-events: none; }
  .bright { display: flex; flex-direction: column; gap: 4px; font-size: 11px; }
  .bright input { width: 100%; accent-color: var(--accent); }
</style>
