export { default as Button } from './button.svelte';
export type { ButtonVariant, ButtonSize } from './button.svelte';
// The Panda `button` recipe, for styling non-<button> elements (e.g. a download
// link) to match. Call as buttonVariants({ variant, size }) -> class string.
export { button as buttonVariants } from 'styled-system/recipes';
