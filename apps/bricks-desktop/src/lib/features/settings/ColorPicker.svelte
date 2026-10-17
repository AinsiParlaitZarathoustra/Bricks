<script lang="ts">
	// Visual palette of one colour: a saturation and brightness area, a hue slider, preset swatches,
	// the hex code, a preview and the contrast checks of the role. It edits one colour and hands the
	// new value to the parent, which stores it per mode.
	import { Input } from '$lib/components/ui/input';
	import {
		hexToHsv,
		hsvToHex,
		normalizeHex,
		type ColorCheck,
		type Hex,
		type Swatch
	} from '$lib/theme/palette';

	let {
		label,
		value,
		foreground,
		sample,
		presets,
		checks,
		onChange
	}: {
		/** Names the palette for assistive technology, e.g. « Accent ». */
		label: string;
		value: Hex;
		/** Text colour shown on the preview. */
		foreground: Hex;
		/** Short text shown on the preview. */
		sample: string;
		presets: readonly Swatch[];
		checks: ColorCheck[];
		onChange: (hex: Hex) => void;
	} = $props();

	const uid = $props.id();
	const fromValue = $derived(hexToHsv(value));
	/**
	 * The hue the user chose. A grey or black colour has no reliable hue (8-bit rounding scrambles it
	 * near zero saturation), so the picker keeps the last one instead of jumping.
	 */
	let hueKept = $state<number | null>(null);
	const hue = $derived(fromValue.s > 0.05 && fromValue.v > 0.05 ? fromValue.h : (hueKept ?? fromValue.h));

	/** Text typed in the hex field, until it is committed (Enter or leaving the field). */
	let typed = $state<string | null>(null);
	const typedInvalid = $derived(typed !== null && normalizeHex(typed) === null);

	let dragging = false;

	function clamp01(x: number): number {
		return Math.min(1, Math.max(0, x));
	}

	function setSaturationValue(s: number, v: number) {
		hueKept = hue;
		onChange(hsvToHex({ h: hue, s, v }));
	}

	function pointAt(event: PointerEvent, area: HTMLElement) {
		const box = area.getBoundingClientRect();
		setSaturationValue(clamp01((event.clientX - box.left) / box.width), clamp01(1 - (event.clientY - box.top) / box.height));
	}

	function onAreaDown(event: PointerEvent) {
		const area = event.currentTarget as HTMLElement;
		dragging = true;
		area.setPointerCapture(event.pointerId);
		pointAt(event, area);
	}

	function onAreaMove(event: PointerEvent) {
		if (dragging) pointAt(event, event.currentTarget as HTMLElement);
	}

	function onAreaUp(event: PointerEvent) {
		dragging = false;
		const area = event.currentTarget as HTMLElement;
		if (area.hasPointerCapture(event.pointerId)) area.releasePointerCapture(event.pointerId);
	}

	function setHue(next: number) {
		// On a grey or black colour the slider must still give a colour: saturation and brightness rise.
		const s = fromValue.s < 0.02 ? 0.6 : fromValue.s;
		const v = fromValue.v < 0.02 ? 0.9 : fromValue.v;
		hueKept = next;
		onChange(hsvToHex({ h: next, s, v }));
	}

	function pick(hex: Hex) {
		hueKept = null;
		onChange(hex);
	}

	function onHexInput(event: Event) {
		typed = (event.currentTarget as HTMLInputElement).value;
	}

	function commitHex() {
		if (typed === null) return;
		const hex = normalizeHex(typed);
		typed = null;
		if (hex) pick(hex);
	}

	function onHexKey(event: KeyboardEvent) {
		if (event.key !== 'Enter') return;
		event.preventDefault();
		commitHex();
	}

	function ratioText(ratio: number): string {
		return `${ratio.toFixed(1).replace('.', ',')} : 1`;
	}
</script>

<div class="flex flex-col gap-3 text-sm">
	<!-- Pointer surface only: the keyboard path is the two sliders below it. -->
	<div
		role="presentation"
		class="relative h-36 w-full cursor-crosshair touch-none overflow-hidden rounded-md border border-border"
		style:background-color={hsvToHex({ h: hue, s: 1, v: 1 })}
		onpointerdown={onAreaDown}
		onpointermove={onAreaMove}
		onpointerup={onAreaUp}
		onpointercancel={onAreaUp}
	>
		<div class="pointer-events-none absolute inset-0" style="background: linear-gradient(to right, #ffffff, rgb(255 255 255 / 0));"></div>
		<div class="pointer-events-none absolute inset-0" style="background: linear-gradient(to top, #000000, rgb(0 0 0 / 0));"></div>
		<span
			class="pointer-events-none absolute size-4 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-md"
			style:left={`${fromValue.s * 100}%`}
			style:top={`${(1 - fromValue.v) * 100}%`}
			style:background-color={value}
		></span>
	</div>

	<div class="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-2 text-xs text-muted-foreground">
		<span id={`${uid}-sat-label`}>Saturation</span>
		<input
			type="range"
			min="0"
			max="100"
			step="1"
			value={Math.round(fromValue.s * 100)}
			oninput={(event) => setSaturationValue(Number(event.currentTarget.value) / 100, fromValue.v)}
			class="ramp-range w-full"
			style:background={`linear-gradient(to right, ${hsvToHex({ h: hue, s: 0, v: fromValue.v })}, ${hsvToHex({ h: hue, s: 1, v: fromValue.v })})`}
			aria-labelledby={`${uid}-sat-label`}
		/>

		<span id={`${uid}-val-label`}>Luminosité</span>
		<input
			type="range"
			min="0"
			max="100"
			step="1"
			value={Math.round(fromValue.v * 100)}
			oninput={(event) => setSaturationValue(fromValue.s, Number(event.currentTarget.value) / 100)}
			class="ramp-range w-full"
			style:background={`linear-gradient(to right, #000000, ${hsvToHex({ h: hue, s: fromValue.s, v: 1 })})`}
			aria-labelledby={`${uid}-val-label`}
		/>

		<span id={`${uid}-hue-label`}>Teinte</span>
		<input
			type="range"
			min="0"
			max="360"
			step="1"
			value={Math.round(hue)}
			oninput={(event) => setHue(Number(event.currentTarget.value))}
			class="hue-range w-full"
			aria-labelledby={`${uid}-hue-label`}
		/>
	</div>

	<div class="flex flex-wrap gap-1.5" role="group" aria-label={`Couleurs proposées, ${label}`}>
		{#each presets as preset (preset.hex)}
			<button
				type="button"
				class="size-6 rounded-md border border-border shadow-xs focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none {value === preset.hex ? 'ring-2 ring-ring ring-offset-1 ring-offset-background' : ''}"
				style:background-color={preset.hex}
				title={preset.label}
				aria-label={preset.label}
				aria-pressed={value === preset.hex}
				onclick={() => pick(preset.hex)}
			></button>
		{/each}
	</div>

	<div class="flex flex-col gap-1.5">
		<Input
			id={`${uid}-hex`}
			value={typed ?? value}
			oninput={onHexInput}
			onchange={commitHex}
			onkeydown={onHexKey}
			onblur={() => (typed = null)}
			autocomplete="off"
			spellcheck={false}
			maxlength={7}
			class="h-8 font-mono text-xs"
			aria-label={`Code couleur, ${label}`}
			aria-invalid={typedInvalid ? 'true' : undefined}
			aria-describedby={typedInvalid ? `${uid}-hex-error` : undefined}
		/>
		{#if typedInvalid}
			<p id={`${uid}-hex-error`} role="alert" class="text-xs text-destructive">Saisissez un code hexadécimal, par exemple #a4512c.</p>
		{/if}
	</div>

	<div class="flex items-center gap-3 rounded-md border border-border p-2">
		<div
			class="flex h-9 min-w-0 flex-1 items-center justify-center truncate rounded-md px-2 text-xs font-medium"
			style:background-color={value}
			style:color={foreground}
			aria-hidden="true"
		>
			{sample}
		</div>
	</div>

	<ul class="flex flex-col gap-1 text-xs" aria-label={`Contrastes, ${label}`}>
		{#each checks as check (check.label)}
			{@const passes = check.ratio >= check.min}
			<li class="flex items-center justify-between gap-2">
				<span class="text-muted-foreground">{check.label}</span>
				<span class={passes ? 'tabular-nums text-foreground' : 'tabular-nums text-warning'}>
					{ratioText(check.ratio)}, {passes ? 'conforme' : `insuffisant (minimum ${ratioText(check.min)})`}
				</span>
			</li>
		{/each}
	</ul>
</div>

<style>
	/* Sliders: the track colour is the gradient (hue rainbow, or a ramp set inline); the thumb is a ring. */
	.hue-range,
	.ramp-range {
		-webkit-appearance: none;
		appearance: none;
		height: 0.75rem;
		border-radius: 9999px;
		outline-offset: 2px;
	}

	.hue-range {
		background: linear-gradient(to right, #ff0000, #ffff00, #00ff00, #00ffff, #0000ff, #ff00ff, #ff0000);
	}

	.hue-range:focus-visible,
	.ramp-range:focus-visible {
		outline: 2px solid var(--ring);
	}

	.hue-range::-webkit-slider-thumb,
	.ramp-range::-webkit-slider-thumb {
		-webkit-appearance: none;
		appearance: none;
		width: 1rem;
		height: 1rem;
		border-radius: 9999px;
		border: 2px solid #ffffff;
		background: transparent;
		box-shadow: 0 0 0 1px rgb(0 0 0 / 0.35);
		cursor: pointer;
	}

	.hue-range::-moz-range-thumb,
	.ramp-range::-moz-range-thumb {
		width: 1rem;
		height: 1rem;
		border-radius: 9999px;
		border: 2px solid #ffffff;
		background: transparent;
		box-shadow: 0 0 0 1px rgb(0 0 0 / 0.35);
		cursor: pointer;
	}
</style>
