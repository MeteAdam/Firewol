<script lang="ts">
  import { getVersion } from "@tauri-apps/api/app";
  import type { AppearancePatch, Pin, SettingsPatch, SettingsView } from "./contract";
  import { COLOR_GROUPS, COLOR_SLOTS, PRESETS, colors, paletteOf, presetClass, type ColorSlot } from "./appearance";
  import { contrastRatio, cssRgb, hexOf, parseHex, type Rgb } from "./color";
  import AdminSettings from "./AdminSettings.svelte";
  import ColorWheel from "./ColorWheel.svelte";
  import DefaultApps from "./DefaultApps.svelte";
  import Icon from "./Icon.svelte";
  import MenuEditor from "./MenuEditor.svelte";
  import StartupSettings from "./StartupSettings.svelte";

  let { settings, pins, current, dark, section = $bindable("appearance"), onappearance, onsave }: {
    settings: SettingsView;
    pins: Pin[];
    /** The folder open in the active tab. */
    current: string;
    dark: boolean;
    section?: string;
    onappearance: (appearance: AppearancePatch) => void;
    /** Saves a patch; rejects with the core's message so sections can show it inline. */
    onsave: (patch: SettingsPatch) => Promise<void>;
  } = $props();

  const SECTIONS = [
    { id: "appearance", name: "Appearance", icon: "palette", keys: "theme preset light dark follow windows colours colors wheel background surface text accent folder label secondary text contrast title bar tabs selection popup error window controls caption buttons side shape fill hover" },
    { id: "startup", name: "Startup", icon: "open", keys: "startup start open launch first folder tab restore last window pinned choose begin resume" },
    { id: "menu", name: "Context menu", icon: "menu", keys: "context menu right click actions hide show reorder custom action executable reset" },
    { id: "search", name: "Search", icon: "search", keys: "search find folder shortcut regex regular expression filters extension size date ntfs volume drive scope index" },
    { id: "transfers", name: "Transfers", icon: "transfer", keys: "transfers copy move verify pause resume cancel speed graph drag drop" },
    { id: "admin", name: "Administrator", icon: "shield", keys: "administrator admin elevation uac run as allowlist executable any" },
    { id: "defaults", name: "Default apps", icon: "window", keys: "default apps file manager explorer replace win e folders open registry restore" },
    { id: "about", name: "About", icon: "info", keys: "about version data settings file license" },
  ] as const;
  const SHAPES = ["rounded", "square", "circle", "minimal"] as const;

  let find = $state("");
  let slot = $state<ColorSlot>("accent");
  let probe = $state<HTMLDivElement>();
  let preview = $state<HTMLDivElement>();
  let warnings = $state<string[]>([]);
  let defaults = $state<Partial<Record<ColorSlot, Rgb | null>>>({});
  let version = $state("");

  const words = $derived(find.trim().toLowerCase().split(/\s+/).filter(Boolean));
  const visible = $derived(SECTIONS.filter((item) => words.every((word) => `${item.name} ${item.keys}`.toLowerCase().includes(word))));
  const appearance = $derived(settings.appearance);
  const css = $derived(presetClass(appearance.preset));
  const presetName = $derived(PRESETS.find(([id]) => id === appearance.preset)?.[2] ?? "the preset");
  const darkCapable = $derived(css === "light" || css === "neu");
  const mode = $derived(appearance.follow_windows ? "follow" : appearance.dark ? "dark" : "light");
  const palette = $derived(paletteOf(appearance));
  const wheelColor = $derived(palette[slot] ?? defaults[slot] ?? [255, 255, 255] as Rgb);

  void getVersion().then((value) => (version = value)).catch(() => (version = ""));

  // Preset defaults come from the same theme.css tokens the window uses, read from a hidden probe.
  $effect(() => {
    void css;
    void dark;
    if (!probe) return;
    const style = getComputedStyle(probe);
    defaults = Object.fromEntries(COLOR_SLOTS.map(([key, , token]) => [key, cssRgb(style.getPropertyValue(`--${token}`))]));
  });

  $effect(() => {
    const customized = Object.keys(palette).length > 0;
    void dark;
    void css;
    if (!preview || !customized) { warnings = []; return; }
    // Read the actual resolved colours after the action has applied the palette.
    const node = preview;
    const frame = requestAnimationFrame(() => {
      const style = getComputedStyle(node);
      const rgb = (token: string) => cssRgb(style.getPropertyValue(`--${token}`));
      const pairs = [
        ["Text", "text", "bg"], ["Settings descriptions", "muted", "surface"],
        ["Input text", "text", "field"], ["Menu text", "text", "surface-2"],
        ["Folder names", "folder-label", "bg"], ["Folder path", "folder-label", "field"],
        ["Active tab", "tab-active-text", "tab-active"], ["Inactive tab", "tab-inactive-text", "tab-inactive"],
        ["Selection", "sel-text", "sel"], ["Accent text", "on-accent", "accent"],
        ["Error text", "danger", "surface"], ["Window controls", "caption-text", "caption-fill"],
      ];
      warnings = pairs.flatMap(([name, foreground, background]) => {
        const text = rgb(foreground);
        const fill = rgb(background) ?? (style.getPropertyValue(`--${background}`).trim() === "transparent" ? rgb("titlebar") ?? rgb("bg") : null);
        return text && fill && contrastRatio(text, fill) < 4.5 ? [name] : [];
      });
    });
    return () => cancelAnimationFrame(frame);
  });

  function update(patch: AppearancePatch) {
    onappearance(patch);
  }

  function setColor(key: ColorSlot, value: Rgb | null) {
    update({ palettes: { [appearance.preset]: { [key]: value } } });
  }

  function setHex(key: ColorSlot, input: HTMLInputElement) {
    const value = parseHex(input.value);
    if (value === undefined) {
      input.setCustomValidity("Enter a colour as #RRGGBB");
      input.reportValidity();
      return;
    }
    input.setCustomValidity("");
    slot = key;
    setColor(key, value);
  }

  function resetColors() {
    update({ palettes: { [appearance.preset]: Object.fromEntries(COLOR_SLOTS.map(([key]) => [key, null])) } });
  }

  function jump() {
    if (visible[0]) section = visible[0].id;
  }

  const shown = (key: ColorSlot) => palette[key] ?? defaults[key] ?? null;
</script>

<section class="settings" aria-label="Settings">
  <div hidden class="fw p-{css}" class:dark bind:this={probe}></div>
  <div hidden class="fw p-{css}" class:dark bind:this={preview} use:colors={appearance}></div>
  <header class="s-head">
    <h1>Settings</h1>
    <span class="muted">Changes apply immediately</span>
    <span class="grow"></span>
    <label class="field find">
      <span class="muted icon"><Icon name="search" /></span>
      <span class="sr">Find a setting</span>
      <input type="search" placeholder="Find a setting" bind:value={find} onkeydown={(event) => { if (event.key === "Enter") jump(); }} />
    </label>
  </header>
  <div class="s-body">
    <nav class="s-nav" aria-label="Settings sections">
      {#each visible as item (item.id)}
        <button class="s-link" class:on={section === item.id} aria-current={section === item.id ? "page" : undefined} onclick={() => (section = item.id)}>
          <Icon name={item.icon} /><span>{item.name}</span>
        </button>
      {:else}
        <p class="muted s-none">No setting matches “{find}”.</p>
      {/each}
    </nav>
    <div class="s-card">
      {#if section === "appearance"}
        <div class="row-head">
          <div>
            <h2>Theme preset</h2>
            <p class="muted">Changes the materials of the whole window. Layout and shortcuts stay the same.</p>
          </div>
          {#if darkCapable}
            <div class="seg text" role="group" aria-label="Light or dark">
              <button class:on={mode === "light"} aria-pressed={mode === "light"} onclick={() => update({ dark: false, follow_windows: false })}>Light</button>
              <button class:on={mode === "dark"} aria-pressed={mode === "dark"} onclick={() => update({ dark: true, follow_windows: false })}>Dark</button>
              <button class:on={mode === "follow"} aria-pressed={mode === "follow"} onclick={() => update({ follow_windows: true })}>Follow Windows</button>
            </div>
          {/if}
        </div>
        <div class="presets">
          {#each PRESETS as [id, presetCss, name] (id)}
            <button class="choice preset" class:on={appearance.preset === id} aria-pressed={appearance.preset === id} onclick={() => update({ preset: id })}>
              <span class="fw p-{presetCss} mini" aria-hidden="true">
                <span class="m-surface"><span class="m-top"><span class="m-dot"></span><span class="m-bar"></span></span><span class="m-line"></span><span class="m-line short"></span></span>
              </span>
              <span class="p-name">{name}{#if appearance.preset === id}<Icon name="check" size={14} />{/if}</span>
            </button>
          {/each}
        </div>
        <hr />
        <div class="split">
          <div>
            <h2>Colours</h2>
            <p class="muted">Overrides for {presetName}. Pick a slot, then a colour on the wheel or type a hex value.</p>
            <div class="colours">
              <ColorWheel color={wheelColor} label={COLOR_SLOTS.find(([key]) => key === slot)?.[1] ?? ""} onpick={(rgb) => setColor(slot, rgb)} />
              <div class="slots">
                {#each COLOR_GROUPS as [group, slots] (group)}
                  <details open={slots.some(([key]) => key === slot)}>
                    <summary>{group}</summary>
                    {#each slots as [key, name] (key)}
                      {@const value = shown(key)}
                      <div class="slot" class:on={slot === key}>
                        <button class="slot-pick" aria-pressed={slot === key} onclick={() => (slot = key)}>
                          <span class="swatch" style:background={value ? hexOf(value) : "transparent"}></span><span>{name}</span>
                        </button>
                        <input class="hex" aria-label={`${name} colour`} spellcheck="false" value={palette[key] ? hexOf(palette[key]) : ""} placeholder={value ? hexOf(value) : "Preset"} onchange={(event) => setHex(key, event.currentTarget)} />
                      </div>
                    {/each}
                  </details>
                {/each}
                <button class="link" onclick={resetColors}>Reset to preset</button>
              </div>
            </div>
            {#if warnings.length}<p class="contrast" aria-live="polite">Low contrast (below 4.5:1) · {warnings.join(", ")}</p>{/if}
          </div>
          <div>
            <h2>Window controls</h2>
            <p class="muted">Minimize, maximize and close buttons.</p>
            <div class="line">
              <span>Side</span>
              <div class="seg text" role="group" aria-label="Window controls side">
                <button class:on={appearance.caption_left} aria-pressed={appearance.caption_left} onclick={() => update({ caption_left: true })}>Left</button>
                <button class:on={!appearance.caption_left} aria-pressed={!appearance.caption_left} onclick={() => update({ caption_left: false })}>Right</button>
              </div>
            </div>
            <div class="shapes">
              {#each SHAPES as shape (shape)}
                <button class="choice shape" class:on={appearance.caption_shape === shape} aria-pressed={appearance.caption_shape === shape} onclick={() => update({ caption_shape: shape })}>
                  <span class="caps {shape}" aria-hidden="true">
                    {#if shape === "minimal"}<Icon name="minimize" size={10} /><Icon name="maximize" size={10} /><Icon name="close" size={10} />
                    {:else}<span class="c1"></span><span class="c2"></span><span class="c3"></span>{/if}
                  </span>
                  <span class="cap-name">{shape[0].toUpperCase() + shape.slice(1)}</span>
                </button>
              {/each}
            </div>
          </div>
        </div>
      {:else if section === "search"}
        <h2>Search</h2>
        <p class="muted">Type in the search field (Ctrl+F) to search the open folder and its subfolders. Results appear when you pause typing and keep their own selection. Esc clears the search.</p>
        <p class="muted">While the field is in use, the <strong>.*</strong> button switches to regular expressions and <strong>Filters</strong> narrows results by extension, size and date modified. Each tab keeps its own search text, filters and scope.</p>
        <p class="muted">The scope chip can search a whole local NTFS drive instead of the folder. Firewol asks first, then Windows asks for administrator approval for a read-only helper that reads the drive's change journal. The drive index lasts for the session and is shared by every window; stop it from the scope chip.</p>
      {:else if section === "transfers"}
        <h2>Transfers</h2>
        <p class="muted">Copies are verified against the source before they count as done. The transfer button in the status bar opens a speed graph with Pause, Resume and Cancel, and keeps the last result until you dismiss it.</p>
        <p class="muted">Drag items onto a folder, the other pane, a tab or a pin to copy them there; hold Shift to move instead, or press Esc to cancel.</p>
        <p class="muted">Ctrl+Z undoes the last move, rename or Recycle Bin operation.</p>
      {:else if section === "about"}
        <h2>About Firewol</h2>
        <dl class="facts">
          <dt>Version</dt><dd>{version || "—"}</dd>
          <dt>Settings file</dt><dd class="mono">%LOCALAPPDATA%\Firewol\explorer.json</dd>
          <dt>License</dt><dd>MIT</dd>
        </dl>
      {:else if section === "menu"}
        <MenuEditor menu={settings.menu} shellItems={settings.shell_items} {onsave} />
      {:else if section === "admin"}
        <AdminSettings elevation={settings.elevation} {onsave} />
      {:else if section === "startup"}
        <StartupSettings startup={settings.startup} {pins} {current} {onsave} />
      {:else if section === "defaults"}
        <DefaultApps />
      {/if}
    </div>
  </div>
</section>

<style>
  .settings { display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; gap: 12px; }
  .s-head { display: flex; align-items: center; gap: 12px; padding: 0 2px; }
  h1 { margin: 0; font-size: 20px; font-weight: 600; }
  h2 { margin: 0 0 2px; font-size: 14px; font-weight: 600; }
  p { margin: 0 0 12px; font-size: 12px; }
  .grow { flex-grow: 1; }
  .find { width: 280px; }
  .icon { display: flex; }
  .s-body { display: flex; flex: 1; min-height: 0; gap: 8px; }
  .s-nav { display: flex; flex-direction: column; gap: 2px; width: 216px; flex: none; }
  .s-link { display: flex; align-items: center; gap: 12px; height: 36px; padding: 0 12px; border: 0; border-radius: var(--radius-s); background: transparent; font-size: 14px; text-align: left; }
  .s-link :global(svg) { color: var(--muted); }
  .s-link.on :global(svg) { color: inherit; }
  .s-link:hover { background: var(--hover); }
  .s-link.on { background: var(--sel); color: var(--sel-text); box-shadow: var(--sel-shadow); font-weight: 600; }
  .s-none { padding: 8px 12px; }
  .s-card { flex: 1; min-width: 0; overflow: auto; padding: 20px 24px; background: var(--surface); border: var(--border-w) solid var(--border); border-radius: var(--radius); box-shadow: var(--shadow); user-select: text; }
  .row-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 16px; }
  .seg.text button { height: 30px; padding: 0 12px; border: 0; border-radius: calc(var(--radius-s) - 2px); background: transparent; font-size: 14px; }
  .seg.text button.on { background: var(--sel); color: var(--sel-text); box-shadow: var(--sel-shadow); font-weight: 600; }
  .choice { display: flex; flex-direction: column; gap: 8px; padding: 8px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--surface-2); text-align: left; }
  .choice:hover { background: var(--hover); }
  .choice.on { outline: 2px solid var(--accent); outline-offset: -1px; }
  .presets { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 12px; }
  .mini { display: flex; align-items: center; justify-content: center; height: 72px; padding: 10px 14px; border-radius: calc(var(--radius-s) - 2px); font-size: 0; user-select: none; }
  .m-surface { display: flex; flex-direction: column; gap: 6px; width: 100%; padding: 8px 10px; background: var(--surface); border: var(--border-w) solid var(--border); border-radius: var(--radius-s); box-shadow: var(--shadow-s); backdrop-filter: var(--blur); }
  .m-top { display: flex; align-items: center; gap: 8px; }
  .m-dot { width: 10px; height: 10px; border-radius: 3px; background: var(--folder); }
  .m-bar { width: 60%; height: 4px; border-radius: 2px; background: var(--accent); }
  .m-line { height: 3px; border-radius: 2px; background: var(--muted); opacity: .35; }
  .m-line.short { width: 70%; }
  .p-name { display: flex; align-items: center; justify-content: space-between; padding: 0 2px; font-size: 14px; font-weight: 600; }
  .p-name :global(svg) { color: var(--accent); }
  hr { margin: 20px 0; border: 0; border-top: 1px solid var(--sep); }
  .split { display: grid; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr)); gap: 32px; }
  .colours { display: flex; gap: 20px; flex-wrap: wrap; }
  .slots { display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 220px; }
  .slot { display: flex; align-items: center; gap: 8px; padding: 0 6px 0 0; border-radius: var(--radius-s); }
  .slot.on { background: var(--sel); color: var(--sel-text); }
  .slot-pick { display: flex; align-items: center; gap: 12px; flex: 1; min-width: 0; height: 34px; padding: 0 8px; border: 0; background: transparent; font-size: 14px; text-align: left; }
  .swatch { flex: none; width: 20px; height: 20px; border: 1px solid var(--border); border-radius: 5px; box-shadow: inset 0 0 0 1px rgba(0, 0, 0, .06); }
  .hex { width: 116px; height: 26px; padding: 0 6px; border: 1px solid transparent; border-radius: 5px; background: transparent; font-family: Consolas, "Cascadia Mono", monospace; font-size: 12px; text-align: right; }
  .slot.on .hex::placeholder { color: inherit; }
  .hex:hover, .hex:focus { border-color: var(--border); background: var(--field); color: var(--text); }
  .hex:invalid { border-color: var(--danger); }
  .contrast { color: var(--danger); }
  summary { padding: 6px 0; font-weight: 600; }
  .link { align-self: flex-start; margin-top: 8px; padding: 4px 8px; border: 0; background: transparent; color: var(--accent); font-size: 14px; }
  .link:hover { text-decoration: underline; }
  .line { display: flex; align-items: center; justify-content: space-between; gap: 12px; min-height: 40px; }
  .shapes { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; margin: 8px 0; }
  .shape { align-items: center; padding: 12px 6px 8px; }
  .caps { display: flex; align-items: center; gap: 4px; height: 14px; }
  .caps span { width: 14px; height: 10px; background: var(--hover); border: 1px solid var(--border); }
  .caps .c3 { background: #c42b1c; border-color: #c42b1c; }
  .caps.rounded span { border-radius: 3px; }
  .caps.circle span { width: 11px; height: 11px; border: 0; border-radius: 50%; }
  .caps.circle .c1 { background: #e6a23c; }
  .caps.circle .c2 { background: #3fae5b; }
  .caps.circle .c3 { background: #e5534b; }
  .caps.minimal { gap: 8px; }
  .cap-name { font-size: 14px; }
  .facts { display: grid; grid-template-columns: max-content 1fr; gap: 8px 24px; margin: 12px 0 0; font-size: 13px; }
  .facts dt { color: var(--muted); }
  .facts dd { margin: 0; }
  .mono { font-family: Consolas, "Cascadia Mono", monospace; font-size: 12px; }
</style>
