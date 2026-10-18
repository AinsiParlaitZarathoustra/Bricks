// Real-browser harness of the assistant-message renderer (Sprint 14.5).
// Dev server only: `npm run dev`, then /tests/harness/markdown.html. Not bundled.
import { mount } from 'svelte'
import '../../src/app.css'
import Harness from './MarkdownHarness.svelte'

mount(Harness, { target: document.getElementById('app')! })
