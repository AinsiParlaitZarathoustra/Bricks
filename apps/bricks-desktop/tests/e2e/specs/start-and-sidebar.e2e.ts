// Start path and project/conversation sidebar. Skeleton: selectors follow the accessible
// names in src/lib/features/navigation/*.svelte at the time of writing. Run only on Linux/Windows.
import { $, browser, expect } from '@wdio/globals';

const LOADING_TEXT = 'Démarrage de Bricks';
const SIDEBAR = 'aside[aria-label="Projets et conversations"]';
const DEMO_PROJECT = 'Projet de démonstration';

/** A button found by its exact visible text. */
function buttonNamed(text: string) {
	return $(`//button[normalize-space()="${text}"]`);
}

/** The project row button, found by the project name it shows. */
function projectRow(name: string) {
	return $(`//ul[@aria-label="Projets"]//button[.//span[normalize-space()="${name}"]]`);
}

describe('Démarrage de Bricks', () => {
	it('quitte l’écran de démarrage', async () => {
		await browser.waitUntil(async () => !(await $('body').getText()).includes(LOADING_TEXT), {
			timeout: 20_000,
			timeoutMsg: 'l’écran « Démarrage de Bricks… » ne disparaît pas'
		});
		expect(await browser.getTitle()).toBe('Bricks');
	});

	it('affiche la barre latérale avec les actions de projet et de conversation', async () => {
		const sidebar = await $(SIDEBAR);
		await sidebar.waitForDisplayed();
		await expect(buttonNamed('Nouveau projet')).toBeDisplayed();
		await expect($('input[type="search"][aria-label^="Rechercher dans les conversations"]')).toBeDisplayed();
	});
});

describe('Projets', () => {
	it('liste le projet enregistré et le sélectionne', async () => {
		const row = await projectRow(DEMO_PROJECT);
		await row.waitForDisplayed();
		await row.click();
		await expect(row).toHaveAttribute('aria-current', 'true');
	});

	it('affiche un état vide quand le projet n’a pas de conversation', async () => {
		await expect($('//h2[normalize-space()="Conversations"]')).toBeDisplayed();
		await expect($('//*[normalize-space()="Aucune conversation"]')).toBeDisplayed();
	});

	it('ouvre le menu d’actions du projet', async () => {
		const trigger = await $(`//button[@aria-label="Actions pour le projet ${DEMO_PROJECT}"]`);
		await trigger.click();
		await expect($('//*[@role="menuitem"][normalize-space()="Renommer…"]')).toBeDisplayed();
		await browser.keys('Escape');
	});
});

describe('Conversations', () => {
	it('démarre une conversation seulement quand un fournisseur est configuré', async function () {
		const start = await buttonNamed('Nouvelle conversation');
		await start.waitForDisplayed();
		// Without providers.toml the button is aria-disabled: nothing to test beyond the blocked state.
		if ((await start.getAttribute('aria-disabled')) === 'true') {
			await expect(start).toHaveAttribute('aria-disabled', 'true');
			this.skip();
		}
		await start.click();
		await expect($('//ul[@aria-label="Conversations du projet"]//button[@data-session-item]')).toBeDisplayed();
	});

	it('renomme une conversation avec la touche F2', async function () {
		const item = await $('//ul[@aria-label="Conversations du projet"]//button[@data-session-item]');
		if (!(await item.isExisting())) this.skip();
		await item.click();
		await browser.keys('F2');
		const field = await $('input[aria-label="Nouveau titre de la conversation"]');
		await field.waitForDisplayed();
		await field.clearValue();
		await field.setValue('Titre renommé');
		await browser.keys('Enter');
		await expect($('//ul[@aria-label="Conversations du projet"]//span[normalize-space()="Titre renommé"]')).toBeDisplayed();
	});
});
