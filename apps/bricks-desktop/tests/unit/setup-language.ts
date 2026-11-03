// Setup of every unit test file: the interface is shown in French here, so the rendered
// assertions keep their French wording and the French catalogue is exercised. The
// default (English) is covered by i18n.test.ts, which resets the module.
import { setLanguage } from '$lib/i18n/i18n.svelte';

setLanguage('fr');
