import { expect, test } from '@playwright/test';

test('starts the hermetic fixture', async ({ page }) => {
	await page.goto('/');

	await expect(page.getByRole('heading', { name: 'Hermetic Playwright fixture' })).toBeVisible();
});
