<script lang="ts">
  // The welcome of a new conversation: the logo, the name, one line, and four suggestions
  // that fill the composer (they never send). Over a slow field of characters. No entrance
  // animation: the screen is there, it does not arrive.
  import ArrowRight from '@lucide/svelte/icons/arrow-right'
  import { t } from '$lib/i18n/i18n.svelte'
  import { theme } from '$lib/theme/theme.svelte'
  import darkTile from '../../../../assets/brand/app-dark.png'
  import lightTile from '../../../../assets/brand/app-light.png'
  import WelcomeField from './WelcomeField.svelte'
  import { STARTERS, starterText } from './starters'

  let { onPick }: { onPick: (text: string) => void } = $props()

  // The tile that contrasts with the theme on screen, as at start-up.
  const logo = $derived(theme.resolved === 'dark' ? lightTile : darkTile)
</script>

<div class="relative flex h-full items-center justify-center overflow-hidden px-6">
  <WelcomeField />
  <div class="relative flex w-full max-w-[440px] flex-col items-center text-center">
    <img
      src={logo}
      alt=""
      width="60"
      height="60"
      draggable="false"
      class="mb-5 size-[60px] rounded-[28%] shadow-[0_12px_50px_-12px_rgb(0_0_0/0.55)] ring-1 ring-element-active"
    />
    <h2
      class="mt-1.5 bg-gradient-to-b from-foreground to-foreground/55 bg-clip-text text-[22px] leading-tight font-semibold tracking-tight text-transparent"
    >
      {t('welcome.title')}
    </h2>
    <p class="mt-1.5 text-base text-muted-foreground">{t('welcome.tagline')}</p>
    <div role="group" aria-label={t('welcome.suggestions')} class="mt-7 grid w-full grid-cols-2 gap-2.5">
      {#each STARTERS as starter (starter.id)}
        {@const Icon = starter.icon}
        <button
          type="button"
          class="group relative flex cursor-pointer flex-col gap-2.5 rounded-[12px] border border-border bg-card p-3 text-left transition-all duration-150 hover:-translate-y-0.5 hover:border-border-strong hover:bg-muted hover:shadow-[0_8px_24px_-12px_rgb(0_0_0/0.45)] active:translate-y-0"
          onclick={() => onPick(starterText(starter))}
        >
          <span
            class="flex size-7 items-center justify-center rounded-lg border border-border-subtle bg-card text-muted-foreground transition-colors group-hover:text-foreground"
          >
            <Icon class="size-[13px]" aria-hidden="true" />
          </span>
          <ArrowRight
            class="absolute top-3 right-3 size-[13px] -translate-x-1 text-muted-foreground opacity-0 transition-all duration-150 group-hover:translate-x-0 group-hover:opacity-100 group-focus-visible:translate-x-0 group-focus-visible:opacity-100"
            aria-hidden="true"
          />
          <span class="text-sm leading-snug font-medium text-muted-foreground transition-colors group-hover:text-foreground">
            {t(starter.label)}
          </span>
        </button>
      {/each}
    </div>
  </div>
</div>
