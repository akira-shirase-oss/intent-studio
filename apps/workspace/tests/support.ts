import AxeBuilder from '@axe-core/playwright';
import { expect, type Page, test } from '@playwright/test';

export { operatorState } from './global-setup.ts';

/** Presses Tab until the focused element matches, as a keyboard user would; fails after `max`. */
export async function tabTo(page: Page, matches: (el: Element) => boolean, max = 40) {
	for (let i = 0; i < max; i += 1) {
		await page.keyboard.press('Tab');
		if (await page.evaluate(`(${matches.toString()})(document.activeElement)`)) return;
	}
	throw new Error('Tab never reached the element');
}

/** The focused element shows a visible focus indicator (a ring or an outline). */
export async function focusIsVisible(page: Page) {
	return page.evaluate(() => {
		const el = document.activeElement;
		if (!el) return false;
		const style = getComputedStyle(el);
		return style.boxShadow !== 'none' || (style.outlineStyle !== 'none' && style.outlineWidth !== '0px');
	});
}

/** The page never scrolls sideways at this width. */
export async function noSidewaysScroll(page: Page) {
	const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
	expect(overflow).toBeLessThanOrEqual(0);
}

/**
 * WCAG 2.1 AA in light and dark: zero serious or critical axe violations, and (when
 * WORKSPACE_SCREENS_DIR is set) a full-page screenshot of each scheme for review.
 */
export async function checkScreen(page: Page, name: string) {
	const width = page.viewportSize()?.width ?? 0;
	for (const scheme of ['light', 'dark'] as const) {
		await page.emulateMedia({ colorScheme: scheme });
		await expect(page.locator('html')).toHaveClass(scheme === 'dark' ? /dark/ : /^(?!.*dark).*$/);
		// Colours change through CSS transitions: check the colours the eye ends up with, not the
		// frames in between.
		await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
		const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
		const serious = results.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
		expect(
			serious.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`),
			`${name} (${scheme}, ${width} px)`
		).toEqual([]);
		await noSidewaysScroll(page);
		const dir = process.env['WORKSPACE_SCREENS_DIR'];
		const file = `${name}-${width}-${scheme}.png`;
		await page.screenshot({ path: dir ? `${dir}/${file}` : test.info().outputPath(file), fullPage: true, animations: 'disabled' });
	}
	await page.emulateMedia({ colorScheme: 'light' });
}
