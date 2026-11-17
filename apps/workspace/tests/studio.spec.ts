import { expect, test } from '@playwright/test';
import { checkScreen, focusIsVisible, operatorState, tabTo } from './support.ts';

// The studio's main tasks, as the operator does them, at 1440 px and at 360 px: add a client,
// record facts and the client's approvals, write a brief, revise it, try to fetch a reference
// on a private address, upload a hostile SVG and a broken file. Each screen is checked by axe in
// light and dark. The journeys run in order within a project and build on each other.

test.describe.configure({ mode: 'serial' });
test.use({ storageState: operatorState });

const fixtures = new URL('../../../fixtures/assets/', import.meta.url).pathname;

function clientName(): string {
	return `Quickbeam Heating (${test.info().project.name})`;
}

function daysAgo(days: number): string {
	return new Date(Date.now() - days * 86_400_000).toISOString().slice(0, 10);
}

test('the operator adds a client and records facts and approvals, by keyboard', async ({ page }) => {
	await page.goto('/workspace/facts');
	await expect(page.getByRole('heading', { name: 'Facts', level: 1 })).toBeVisible();

	// A new client, by keyboard: Tab to the button, Enter, type, Enter.
	await tabTo(page, (el) => el.textContent?.trim() === 'Add a client');
	expect(await focusIsVisible(page)).toBe(true);
	await page.keyboard.press('Enter');
	await expect(page.getByLabel('Client’s business name')).toBeFocused();
	await page.keyboard.type(clientName());
	await page.keyboard.press('Enter');
	await expect(page.getByLabel('Client', { exact: true })).toHaveValue(/.+/);
	await expect(page.getByLabel('Client', { exact: true }).locator('option:checked')).toHaveText(clientName());
	await expect(page.getByText(`No facts yet for ${clientName()}`)).toBeVisible();
	await checkScreen(page, 'facts-empty');

	// A customer review, keyboard only from the Add button.
	await page.getByRole('button', { name: 'Add a fact' }).first().focus();
	await page.keyboard.press('Enter');
	await expect(page.getByRole('heading', { name: `Add a fact for ${clientName()}` })).toBeVisible();
	await expect(page.getByLabel('Kind')).toBeFocused();
	await page.getByLabel('Kind').selectOption('testimonial');
	await page.getByLabel('What it says').focus();
	await page.keyboard.type('“Fixed our boiler the same afternoon.” — Sam, Leeds');
	await page.keyboard.press('Tab');
	await page.keyboard.type('Google review, 2 November');
	await page.keyboard.press('Enter');
	await expect(page.getByRole('status')).toContainText('waiting for the client’s approval');
	const review = page.getByRole('listitem').filter({ hasText: 'Fixed our boiler the same afternoon' });
	await expect(review.getByText('Waiting for approval')).toBeVisible();

	// An approval the server refuses is explained in plain words, at the field.
	await review.getByRole('button', { name: 'Record approval' }).click();
	await review.getByLabel('Approved by').fill('Pat Quickbeam, owner');
	await review.getByLabel('Approved on').fill(daysAgo(0));
	await review.getByLabel('Review by (optional)').fill(daysAgo(3));
	await review.getByRole('button', { name: 'Record approval' }).click();
	await expect(review.getByRole('alert')).toContainText('The review date must be after the approval date.');
	await checkScreen(page, 'facts-approval-refused');
	// Then recorded properly: approved until the kind's longest date.
	await review.getByLabel('Review by (optional)').fill('');
	await review.getByRole('button', { name: 'Record approval' }).click();
	await expect(review.getByText(/^Approved until /)).toBeVisible();

	// A price approved 40 days ago has already run out (prices last 30 days).
	await page.getByRole('button', { name: 'Add a fact' }).click();
	await page.getByLabel('Kind').selectOption('price');
	await page.getByLabel('What it says').fill('Boiler service from £80');
	await page.getByLabel('Where it came from').fill('Price list, October');
	await page.getByRole('button', { name: 'Save fact' }).click();
	const price = page.getByRole('listitem').filter({ hasText: 'Boiler service from £80' });
	await price.getByRole('button', { name: 'Record approval' }).click();
	await price.getByLabel('Approved by').fill('Pat Quickbeam, owner');
	await price.getByLabel('Approved on').fill(daysAgo(40));
	await price.getByRole('button', { name: 'Record approval' }).click();
	await expect(price.getByText(/^Approval ran out after /)).toBeVisible();

	// A contact fact with a phone link, then withdrawn.
	await page.getByRole('button', { name: 'Add a fact' }).click();
	await page.getByLabel('Kind').selectOption('contact_details');
	await page.getByLabel('What it says').fill('Call 0113 496 0000');
	await page.getByLabel('Where it came from').fill('The van');
	await page.getByLabel('Type').selectOption('tel');
	await page.getByLabel('Number or address').fill('0113 496 0000');
	await page.getByRole('button', { name: 'Save fact' }).click();
	const phone = page.getByRole('listitem').filter({ hasText: 'Call 0113 496 0000' });
	await expect(phone.getByText('Links to tel:01134960000')).toBeVisible();
	await phone.getByRole('button', { name: 'Withdraw' }).click();
	await phone.getByRole('button', { name: 'Withdraw' }).click();
	await expect(phone.getByText('Withdrawn', { exact: true })).toBeVisible();

	// The filter narrows the list.
	await page.getByLabel('Show').selectOption('stale');
	await expect(page.getByRole('list', { name: `Facts for ${clientName()}` }).getByRole('listitem')).toHaveCount(1);
	await page.getByLabel('Show').selectOption('all');
	await checkScreen(page, 'facts');
});

test('the operator writes a brief, is told what is missing, saves and revises it', async ({ page }) => {
	await page.goto('/workspace/briefs');
	await page.getByLabel('Client', { exact: true }).selectOption({ label: clientName() });
	await expect(page.getByText(`No brief yet for ${clientName()}`)).toBeVisible();
	await checkScreen(page, 'briefs-empty');

	await page.getByRole('link', { name: 'Write the brief' }).click();
	await expect(page.getByRole('heading', { name: 'New brief' })).toBeVisible();
	await page.getByLabel('Client', { exact: true }).selectOption({ label: clientName() });
	// Only the approved, current review can be chosen; the stale price and the withdrawn phone
	// cannot.
	await expect(page.getByRole('checkbox')).toHaveCount(1);

	// Saved empty: every missing part is named in plain words and linked to its field.
	await page.getByRole('button', { name: 'Save the brief' }).click();
	const summary = page.getByRole('alert').filter({ hasText: 'The brief was not saved' });
	await expect(summary).toContainText('Business name: This is required.');
	await expect(summary).toContainText('Service 1 (name): This is required.');
	await expect(page.getByLabel('Business name, as the site should say it')).toHaveAttribute('aria-invalid', 'true');
	await checkScreen(page, 'brief-form-problems');
	await summary.getByRole('link', { name: 'Business name' }).click();
	await expect(page.getByLabel('Business name, as the site should say it')).toBeFocused();

	await page.getByLabel('Business name, as the site should say it').fill('Quickbeam Heating');
	await page.getByLabel('What the client wants the site to achieve').fill('More boiler-service quote requests from nearby homes.');
	await page.getByLabel('Who it is for').fill('Home owners within ten miles of Leeds.');
	await page.locator('#service-name-0').fill('Boiler service');
	await page.locator('#service-description-0').fill('Annual check and clean.');
	await page.getByRole('button', { name: 'Add a service' }).click();
	await page.locator('#service-name-1').fill('Boiler repair');
	await page.getByLabel('How the site should sound (optional)').fill('Plain and friendly');
	await page.getByLabel('The client’s own web addresses (optional, one per line)').fill('quickbeam.example');
	await page.getByRole('button', { name: 'Add a reference site' }).click();
	await page.getByLabel('Reference 1: address').fill('http://169.254.169.254/latest/meta-data/');
	await page.getByLabel('Notes: what to take from it').fill('The old site, as the client says');
	await page.getByRole('checkbox').check();
	await page.getByRole('button', { name: 'Save the brief' }).click();

	await expect(page.getByRole('heading', { name: 'Quickbeam Heating', level: 1 })).toBeVisible();
	await expect(page.getByRole('status')).toContainText('Saved as revision 1.');
	await expect(page.getByText('Fixed our boiler the same afternoon')).toBeVisible();

	// The reference points at the cloud metadata address: refused, explained, recorded.
	await page.getByRole('button', { name: 'Fetch a copy' }).click();
	await expect(page.getByRole('status').filter({ hasText: 'Not fetched' })).toContainText('private, local or reserved network');
	await page.getByText(/Every fetch, newest first/).click();
	await expect(page.getByText(/refused \(address_forbidden\)/)).toBeVisible();
	await checkScreen(page, 'brief-view');

	// A revision.
	await page.getByRole('link', { name: 'Edit' }).click();
	await expect(page.getByRole('heading', { name: 'Edit the brief for Quickbeam Heating' })).toBeVisible();
	await page.getByLabel('Who it is for').fill('Home owners and landlords within ten miles of Leeds.');
	await page.getByRole('button', { name: 'Save revision 2' }).click();
	await expect(page.getByRole('status')).toContainText('Saved as revision 2.');
	await expect(page.getByText('Home owners and landlords')).toBeVisible();
	await page.getByLabel('Revision').selectOption('1');
	await expect(page.getByText('Revision 1 of 2')).toBeVisible();
	await expect(page.getByText('Home owners within ten miles of Leeds.')).toBeVisible();
	await expect(page.getByRole('link', { name: 'Edit' })).toHaveCount(0);

	await page.goto('/workspace/briefs');
	await page.getByLabel('Client', { exact: true }).selectOption({ label: clientName() });
	await expect(page.getByText('Revision 2')).toBeVisible();
	await checkScreen(page, 'briefs');
});

test('uploads: a hostile SVG is cleaned, a broken file is refused in plain words', async ({ page }) => {
	await page.goto('/workspace/assets');
	await page.getByLabel('Client', { exact: true }).selectOption({ label: clientName() });
	await expect(page.getByText(`No files yet for ${clientName()}`)).toBeVisible();
	await checkScreen(page, 'assets-empty');

	// Without a licence: the server names the field.
	await page.getByLabel(/^File \(SVG/).setInputFiles(`${fixtures}clean-logo.svg`);
	await page.getByLabel('Where it came from').fill('Sent by the owner');
	await page.getByRole('button', { name: 'Upload' }).click();
	await expect(page.locator('#asset-licence-error')).toHaveText('This is required.');

	await page.getByLabel(/^File \(SVG/).setInputFiles(`${fixtures}hostile.svg`);
	await page.getByLabel('Licence: who owns it and how it may be used').fill('Owned by the client');
	await page.getByLabel('Where it came from').fill('Sent by the owner by e-mail');
	await page.getByRole('button', { name: 'Upload' }).click();
	await expect(page.getByRole('status')).toContainText('17 unsafe parts were removed');
	await expect(page.getByRole('img', { name: 'Preview of hostile.svg' })).toBeVisible();
	await expect(page.getByText('17 unsafe parts removed')).toBeVisible();

	await page.getByLabel(/^File \(SVG/).setInputFiles(`${fixtures}not-a-png.png`);
	await page.getByLabel('Licence: who owns it and how it may be used').fill('Owned by the client');
	await page.getByLabel('Where it came from').fill('Sent by the owner');
	await page.getByRole('button', { name: 'Upload' }).click();
	await expect(page.getByRole('alert').filter({ hasText: 'not saved' })).toContainText('The file’s contents are not the type its name says.');
	await checkScreen(page, 'assets');
});

test('the inbox passes the same checks', async ({ page }) => {
	await page.goto('/workspace/');
	await expect(page.getByRole('heading', { name: 'Lead inbox' })).toBeVisible();
	await checkScreen(page, 'inbox');
});

test.describe('signed out', () => {
	test.use({ storageState: { cookies: [], origins: [] } });

	test('the sign-in screen passes the same checks', async ({ page }) => {
		await page.goto('/workspace/');
		await expect(page.getByRole('heading', { name: 'Operator sign-in' })).toBeVisible();
		await checkScreen(page, 'sign-in');
	});
});
