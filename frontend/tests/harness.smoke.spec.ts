import { expect, test } from '@playwright/test';
import { FIXTURE } from './ports';

test('starts the hermetic fixture', async ({ page }) => {
	await page.goto(FIXTURE);

	await expect(page.getByRole('heading', { name: 'Hermetic Playwright fixture' })).toBeVisible();
});
