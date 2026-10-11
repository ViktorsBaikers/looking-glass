import { cleanup, fireEvent, render } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { createRawSnippet } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import Button from './button/button.svelte';

const label = createRawSnippet(() => ({ render: () => '<span>Save</span>' }));

afterEach(() => {
	cleanup();
	document.body.innerHTML = '';
});

/** A form with a text field and the Button as its submit button. */
function inForm(props: Record<string, unknown>) {
	const form = document.createElement('form');
	const field = document.createElement('input');
	form.append(field);
	document.body.append(form);
	const onsubmit = vi.fn((event: Event) => event.preventDefault());
	form.addEventListener('submit', onsubmit);
	const { container } = render(Button, { target: form, props: { type: 'submit', children: label, ...props } });
	return { button: container.querySelector('button')!, field, onsubmit };
}

// A real browser blurs a focused control the moment it becomes disabled
// (jsdom does not), so loading is busy + aria-disabled, never native disabled.
it('marks a loading button busy and unavailable without disabling it', () => {
	const { button } = inForm({ loading: true });
	expect(button.disabled).toBe(false);
	expect(button.getAttribute('aria-busy')).toBe('true');
	expect(button.getAttribute('aria-disabled')).toBe('true');
	// Still hit-testable, so a press lands on it instead of the page behind.
	expect(button.style.pointerEvents).toBe('auto');
});

it('swallows clicks, keys and implicit submission while loading', async () => {
	const onclick = vi.fn();
	const { button, field, onsubmit } = inForm({ loading: true, onclick });
	const user = userEvent.setup();
	await user.click(button);
	button.focus();
	await user.keyboard('{Enter}');
	await user.keyboard(' ');
	await user.type(field, 'x{Enter}');
	expect(onclick).not.toHaveBeenCalled();
	expect(onsubmit).not.toHaveBeenCalled();
});

it('activates normally when not loading', async () => {
	const onclick = vi.fn();
	const { button, onsubmit } = inForm({ onclick });
	await fireEvent.click(button);
	expect(onclick).toHaveBeenCalledTimes(1);
	expect(onsubmit).toHaveBeenCalledTimes(1);
	expect(button.hasAttribute('aria-busy')).toBe(false);
	expect(button.hasAttribute('aria-disabled')).toBe(false);
});

// Forms pass disabled={!canSubmit} with !submitting inside canSubmit, so while
// loading an explicit disabled is on too; loading must still win.
it('stays focusable when loading overlaps an explicit disabled, and still swallows activation', async () => {
	const onclick = vi.fn();
	const { button, field, onsubmit } = inForm({ loading: true, disabled: true, onclick });
	expect(button.disabled).toBe(false);
	expect(button.getAttribute('aria-busy')).toBe('true');
	expect(button.getAttribute('aria-disabled')).toBe('true');
	const user = userEvent.setup();
	await user.click(button);
	button.focus();
	await user.keyboard('{Enter}');
	await user.keyboard(' ');
	await user.type(field, 'x{Enter}');
	expect(onclick).not.toHaveBeenCalled();
	expect(onsubmit).not.toHaveBeenCalled();
});

it('keeps an explicit disabled as native disabled', () => {
	const { button } = inForm({ disabled: true });
	expect(button.disabled).toBe(true);
});
