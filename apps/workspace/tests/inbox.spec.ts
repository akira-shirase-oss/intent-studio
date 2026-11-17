import { expect, test } from '@playwright/test';

// Screenshots are kept for review only; the assertions are what pass or fail.

const password = process.env['WORKSPACE_PASSWORD'] ?? '';

test('the operator signs in with the keyboard alone, reads the inbox and signs out', async ({ page }) => {
	await page.goto('/workspace/');
	await expect(page.getByRole('heading', { name: 'Operator sign-in' })).toBeVisible();

	// A wrong password is refused and announced.
	await page.getByLabel('Password').fill('not the password');
	await page.keyboard.press('Enter');
	await expect(page.getByRole('alert')).toContainText('That password is not right.');
	await expect(page.getByLabel('Password')).toHaveAttribute('aria-invalid', 'true');

	// Keyboard only: focus the field, type, submit with Enter.
	await page.getByLabel('Password').fill('');
	await page.getByLabel('Password').focus();
	await page.keyboard.type(password);
	await page.keyboard.press('Enter');

	await expect(page.getByRole('heading', { name: 'Lead inbox' })).toBeVisible();
	if (test.info().project.name === 'mobile') {
		// A phone gets one card per lead, with every field on screen.
		await expect(page.getByRole('list', { name: 'Leads, newest first' }).getByRole('listitem')).toHaveCount(3);
	} else {
		await expect(page.getByRole('row')).toHaveCount(4); // header + three leads
	}
	// Only the layout for this screen is visible; the other is in the DOM but hidden.
	const visible = (text: string) => page.getByText(text).filter({ visible: true });
	await expect(visible('E2E Newest')).toBeVisible();
	await expect(visible('e2e.oldest@example.test')).toBeVisible();
	await expect(visible('Notified')).toHaveCount(3);
	for (const status of await visible('Notified').all()) await expect(status).toBeInViewport({ ratio: 1 });
	await page.screenshot({ path: test.info().outputPath('inbox.png'), fullPage: true });

	// The page never scrolls sideways; a wide table scrolls inside its own frame.
	const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
	expect(overflow).toBeLessThanOrEqual(0);

	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page.getByRole('heading', { name: 'Operator sign-in' })).toBeVisible();
	// The session is gone on the server too.
	const after = await page.request.get('/v1/operator/leads');
	expect(after.status()).toBe(401);
});

test('the workspace runs under a content security policy and cannot be framed', async ({ page }) => {
	await page.addInitScript(() => {
		const store = window as unknown as { __csp?: string[] };
		store.__csp = [];
		document.addEventListener('securitypolicyviolation', (event) => store.__csp?.push(event.violatedDirective));
	});
	const response = await page.goto('/workspace/');
	const headers = response?.headers() ?? {};
	expect(headers['content-security-policy']).toContain("frame-ancestors 'none'");
	expect(headers['content-security-policy']).toContain("object-src 'none'");
	expect(headers['referrer-policy']).toBe('no-referrer');
	expect(headers['x-frame-options']).toBe('DENY');
	// Scripts are pinned by hash in the page's own policy: an injected inline script would not run.
	const meta = await page.locator('meta[http-equiv="content-security-policy"]').getAttribute('content');
	expect(meta).toContain("script-src 'self' 'sha256-");
	// The app still boots under the policy, without a single violation.
	await expect(page.getByRole('heading', { name: 'Operator sign-in' })).toBeVisible();
	expect(await page.evaluate(() => (window as unknown as { __csp?: string[] }).__csp ?? [])).toEqual([]);
	// A deep link is served by the fallback page under the same rules.
	const deep = await page.goto('/workspace/some/deep/link');
	expect(deep?.headers()['content-security-policy']).toContain("frame-ancestors 'none'");
});
