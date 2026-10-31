<script lang="ts">
  // While Bricks starts: its logo, breathing in a soft shadow. No text. The logo
  // contrasts with the theme last shown: dark tile on light, light tile on dark.
  import darkTile from '../../assets/brand/app-dark.png'
  import lightTile from '../../assets/brand/app-light.png'
  import { theme } from '$lib/theme/theme.svelte'

  const src = $derived(theme.resolved === 'dark' ? lightTile : darkTile)
</script>

<div role="status" aria-label="Démarrage de Bricks" class="grid h-screen place-items-center">
  <img {src} alt="" width="96" height="96" draggable="false" class="logo size-24 select-none" />
</div>

<style>
  .logo {
    animation: breathe 2.4s ease-in-out infinite;
  }

  @keyframes breathe {
    0%,
    100% {
      transform: scale(1);
      filter: drop-shadow(0 2px 6px rgb(0 0 0 / 0.08)) drop-shadow(0 4px 14px rgb(0 113 227 / 0.12));
    }
    50% {
      transform: scale(1.03);
      filter: drop-shadow(0 4px 10px rgb(0 0 0 / 0.1)) drop-shadow(0 8px 28px rgb(0 113 227 / 0.28));
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .logo {
      animation: none;
      filter: drop-shadow(0 4px 14px rgb(0 113 227 / 0.18));
    }
  }
</style>
