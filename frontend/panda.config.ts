import { defineConfig } from '@pandacss/dev';
import { pluginSvelte } from '@pandacss/plugin-svelte';

// Colour values come 1:1 from the Stitch design exports
// (design/stitch/network-diagnostics.html = dark, network-diagnostics-light.html
// = light). `base` is the light scheme; `_dark` activates under the `.dark`
// class that the pre-paint script in app.html toggles on <html>.
export default defineConfig({
	preflight: true,
	plugins: [pluginSvelte()],
	include: ['./src/**/*.{js,ts,svelte}'],
	exclude: [],
	// .svelte extraction is unreliable (the svelte→tsx transform leaves invalid TSX
	// in templates), so pre-generate every recipe's full variant set. One-off
	// css() atomics live in src/lib/styles.ts (.ts extracts reliably).
	staticCss: {
		recipes: {
			button: ['*'],
			badge: ['*'],
			input: ['*'],
			checkboxCard: ['*'],
			field: ['*'],
			card: ['*'],
			dialog: ['*'],
			tabs: ['*'],
			checkbox: ['*'],
			tooltip: ['*'],
			select: ['*'],
			toast: ['*']
		}
	},
	outdir: 'styled-system',
	theme: {
		keyframes: {
			spin: { to: { transform: 'rotate(360deg)' } },
			'fade-in': { from: { opacity: 0 }, to: { opacity: 1 } },
			'content-in': {
				from: { opacity: 0, transform: 'scale(0.96) translateY(4px)' },
				to: { opacity: 1, transform: 'scale(1) translateY(0)' }
			}
		},
		tokens: {
			fonts: {
				sans: { value: ["'Plus Jakarta Sans Variable'", "'Plus Jakarta Sans'", 'sans-serif'] },
				mono: { value: ["'JetBrains Mono Variable'", "'JetBrains Mono'", 'monospace'] }
			},
			// Base radius is 8px (`md`); design exports round cards at 12–16px.
			radii: {
				sm: { value: '4px' },
				md: { value: '8px' },
				lg: { value: '12px' },
				xl: { value: '16px' },
				full: { value: '9999px' }
			},
			shadows: {
				glow: { value: '0 0 8px #10b981' },
				'glow-md': { value: '0 0 15px rgba(16, 185, 129, 0.3)' },
				popup: { value: '0 12px 32px rgba(0, 0, 0, 0.35)' }
			},
			animations: {
				spin: { value: 'spin 1s linear infinite' },
				'fade-in': { value: 'fade-in 150ms ease-out' },
				'content-in': { value: 'content-in 150ms ease-out' }
			}
		},
		semanticTokens: {
			colors: {
				background: { value: { base: '#f8f9ff', _dark: '#0b1326' } },
				surface: { value: { base: '#f8f9ff', _dark: '#0b1326' } },
				'surface-container-lowest': { value: { base: '#ffffff', _dark: '#060e20' } },
				'surface-container-low': { value: { base: '#eff4ff', _dark: '#131b2e' } },
				'surface-container': { value: { base: '#e8ecf8', _dark: '#171f33' } },
				'surface-container-high': { value: { base: '#e2e7f2', _dark: '#222a3d' } },
				'surface-container-highest': { value: { base: '#dce1ec', _dark: '#2d3449' } },
				'surface-variant': { value: { base: '#e1e2ec', _dark: '#2d3449' } },
				'surface-bright': { value: { base: '#f8f9ff', _dark: '#31394d' } },
				'surface-dim': { value: { base: '#cbdbf5', _dark: '#0b1326' } },
				'on-surface': { value: { base: '#191c20', _dark: '#dae2fd' } },
				'on-surface-variant': { value: { base: '#44474e', _dark: '#bbcabf' } },
				outline: { value: { base: '#757780', _dark: '#86948a' } },
				'outline-variant': { value: { base: '#c4c6d0', _dark: '#3c4a42' } },
				primary: { value: { base: '#10b981', _dark: '#4edea3' } },
				'on-primary': { value: { base: '#ffffff', _dark: '#003824' } },
				'primary-container': { value: { base: '#6ffbbe', _dark: '#10b981' } },
				'on-primary-container': { value: { base: '#002114', _dark: '#00422b' } },
				'primary-fixed': { value: '#6ffbbe' },
				'primary-fixed-dim': { value: '#4edea3' },
				'secondary-container': { value: { base: '#dce2f9', _dark: '#0b513d' } },
				'on-secondary-container': { value: { base: '#151b2c', _dark: '#83c2a9' } },
				tertiary: { value: { base: '#81429f', _dark: '#45dfa4' } },
				error: { value: { base: '#ba1a1a', _dark: '#ffb4ab' } },
				'on-error': { value: { base: '#ffffff', _dark: '#690005' } },
				'error-container': { value: { base: '#ffdad6', _dark: '#93000a' } },
				'on-error-container': { value: { base: '#410002', _dark: '#ffdad6' } },
				// Not in the Stitch palette; needed for the PENDING badge (spec #52).
				warning: { value: { base: '#d97706', _dark: '#fbbf24' } },
				'inverse-surface': { value: { base: '#2e3036', _dark: '#dae2fd' } },
				'inverse-on-surface': { value: { base: '#eff4ff', _dark: '#283044' } }
			}
		},
		textStyles: {
			'headline-lg': {
				value: { fontSize: '48px', lineHeight: '56px', fontWeight: 700, letterSpacing: '-0.02em' }
			},
			'headline-md': {
				value: { fontSize: '32px', lineHeight: '40px', fontWeight: 600, letterSpacing: '-0.01em' }
			},
			'headline-sm': { value: { fontSize: '24px', lineHeight: '32px', fontWeight: 600 } },
			'headline-mobile': {
				value: { fontSize: '32px', lineHeight: '40px', fontWeight: 700, letterSpacing: '-0.01em' }
			},
			'body-lg': { value: { fontSize: '18px', lineHeight: '28px', fontWeight: 400 } },
			'body-md': { value: { fontSize: '16px', lineHeight: '24px', fontWeight: 400 } },
			'body-sm': { value: { fontSize: '14px', lineHeight: '20px', fontWeight: 400 } },
			'label-md': {
				value: { fontSize: '14px', lineHeight: '20px', fontWeight: 600, letterSpacing: '0.05em' }
			},
			'label-sm': {
				value: { fontSize: '12px', lineHeight: '16px', fontWeight: 600, letterSpacing: '0.05em' }
			},
			'mono-data': {
				value: { fontFamily: ['mono'], fontSize: '14px', lineHeight: '22px', fontWeight: 400 }
			}
		},
		recipes: {
			button: {
				className: 'button',
				base: {
					display: 'inline-flex',
					alignItems: 'center',
					justifyContent: 'center',
					gap: '8px',
					whiteSpace: 'nowrap',
					borderRadius: 'md',
					textStyle: 'label-md',
					cursor: 'pointer',
					transitionProperty: 'background, color, border-color, box-shadow',
					transitionDuration: '150ms',
					borderStyle: 'solid',
					borderWidth: '0',
					_disabled: { opacity: 0.5, cursor: 'not-allowed', pointerEvents: 'none' },
					_focusVisible: { outline: '2px solid {colors.primary}', outlineOffset: '2px' },
					'& svg': { width: '20px', height: '20px', flexShrink: 0 }
				},
				variants: {
					variant: {
						primary: {
							background: 'primary-container',
							color: 'on-primary-container',
							boxShadow: 'glow-md',
							_hover: { background: 'primary', color: 'on-primary' }
						},
						secondary: {
							background: 'surface-container-high',
							color: 'on-surface',
							borderWidth: '1px',
							borderColor: 'outline-variant',
							_hover: { borderColor: 'primary', color: 'primary' }
						},
						ghost: {
							background: 'transparent',
							color: 'on-surface-variant',
							_hover: { background: 'surface-container-highest', color: 'primary' }
						},
						danger: {
							background: 'transparent',
							color: 'error',
							_hover: { background: 'error-container', color: 'on-error-container' }
						}
					},
					size: {
						sm: { minHeight: '36px', padding: '6px 12px', textStyle: 'label-sm' },
						md: { minHeight: '44px', padding: '10px 20px' },
						lg: { minHeight: '48px', padding: '12px 24px' },
						icon: { minHeight: '40px', minWidth: '40px', padding: '0' }
					}
				},
				defaultVariants: { variant: 'primary', size: 'md' }
			},
			badge: {
				className: 'badge',
				base: {
					display: 'inline-flex',
					alignItems: 'center',
					gap: '6px',
					borderRadius: 'full',
					padding: '2px 10px',
					textStyle: 'label-sm',
					textTransform: 'uppercase',
					fontWeight: 700,
					borderWidth: '1px',
					borderStyle: 'solid'
				},
				variants: {
					tone: {
						neutral: {
							background: 'surface-container-highest',
							color: 'on-surface-variant',
							borderColor: 'outline-variant'
						},
						success: { background: 'primary/10', color: 'primary', borderColor: 'primary/30' },
						warning: { background: 'warning/10', color: 'warning', borderColor: 'warning/30' },
						danger: { background: 'error/10', color: 'error', borderColor: 'error/30' }
					},
					// `lg` is the Locations card state pill: sentence case, label-md.
					size: {
						sm: {},
						lg: {
							gap: '8px',
							padding: '8px 16px',
							textStyle: 'label-md',
							textTransform: 'none',
							fontWeight: 600
						}
					}
				},
				defaultVariants: { tone: 'neutral', size: 'sm' }
			},
			input: {
				className: 'input',
				base: {
					width: '100%',
					minHeight: '44px',
					background: 'surface-container-high',
					color: 'on-surface',
					borderWidth: '1px',
					borderStyle: 'solid',
					borderColor: 'outline-variant',
					borderRadius: 'md',
					padding: '10px 12px',
					textStyle: 'body-md',
					transitionProperty: 'background, border-color, box-shadow',
					transitionDuration: '150ms',
					_placeholder: { color: 'on-surface-variant' },
					_focus: {
						outline: 'none',
						borderColor: 'primary',
						boxShadow: '0 0 0 1px {colors.primary}'
					},
					_disabled: { opacity: 0.6, cursor: 'not-allowed' }
				},
				variants: {
					invalid: {
						true: {
							borderColor: 'error',
							_focus: { borderColor: 'error', boxShadow: '0 0 0 1px {colors.error}' }
						}
					},
					mono: { true: { fontFamily: 'mono' } }
				}
			},
			checkboxCard: {
				className: 'checkbox-card',
				base: {
					display: 'flex',
					alignItems: 'center',
					gap: '12px',
					padding: '12px',
					borderRadius: 'md',
					borderWidth: '1px',
					borderStyle: 'solid',
					borderColor: 'surface-bright',
					background: 'surface-container',
					cursor: 'pointer',
					transitionProperty: 'background, border-color, color',
					transitionDuration: '150ms',
					_hover: { background: 'surface-container-high', borderColor: 'primary' }
				}
			}
		},
		slotRecipes: {
			field: {
				className: 'field',
				slots: ['root', 'label', 'control', 'error', 'hint'],
				base: {
					root: { display: 'flex', flexDirection: 'column', gap: '4px' },
					label: { textStyle: 'label-md', color: 'on-surface-variant' },
					control: {},
					error: { textStyle: 'body-sm', color: 'error' },
					hint: { textStyle: 'body-sm', color: 'on-surface-variant' }
				}
			},
			card: {
				className: 'card',
				slots: ['root', 'header', 'title', 'description', 'content', 'footer'],
				base: {
					root: {
						background: 'surface-container-low',
						color: 'on-surface',
						borderWidth: '1px',
						borderStyle: 'solid',
						borderColor: 'outline-variant',
						borderRadius: 'xl',
						padding: '24px'
					},
					header: { display: 'flex', flexDirection: 'column', gap: '4px', marginBottom: '16px' },
					title: { textStyle: 'headline-sm', color: 'on-surface' },
					description: { textStyle: 'body-sm', color: 'on-surface-variant' },
					content: {},
					footer: { display: 'flex', alignItems: 'center', gap: '8px', marginTop: '16px' }
				}
			},
			dialog: {
				className: 'dialog',
				slots: ['backdrop', 'positioner', 'content', 'title', 'description', 'closeTrigger'],
				base: {
					backdrop: {
						position: 'fixed',
						inset: '0',
						background: 'black/60',
						zIndex: 50,
						animation: 'fade-in'
					},
					positioner: {
						position: 'fixed',
						inset: '0',
						display: 'flex',
						alignItems: 'center',
						justifyContent: 'center',
						padding: '16px',
						zIndex: 50,
						overflowY: 'auto'
					},
					content: {
						position: 'relative',
						background: 'surface-container-low',
						color: 'on-surface',
						borderWidth: '1px',
						borderStyle: 'solid',
						borderColor: 'outline-variant',
						borderRadius: 'xl',
						padding: '24px',
						width: '100%',
						maxWidth: '512px',
						maxHeight: '90vh',
						overflowY: 'auto',
						boxShadow: 'popup',
						outline: 'none',
						animation: 'content-in'
					},
					title: { textStyle: 'headline-sm', marginBottom: '4px' },
					description: { textStyle: 'body-sm', color: 'on-surface-variant', marginBottom: '16px' },
					closeTrigger: {
						position: 'absolute',
						top: '12px',
						right: '12px',
						display: 'flex',
						padding: '4px',
						borderRadius: 'md',
						color: 'on-surface-variant',
						cursor: 'pointer',
						background: 'transparent',
						border: 'none',
						_hover: { color: 'on-surface', background: 'surface-container-high' },
						_focusVisible: { outline: '2px solid {colors.primary}', outlineOffset: '1px' }
					}
				}
			},
			tabs: {
				className: 'tabs',
				slots: ['root', 'list', 'trigger', 'content'],
				base: {
					root: { minWidth: '0' },
					list: {
						display: 'flex',
						gap: '24px',
						borderBottomWidth: '1px',
						borderBottomStyle: 'solid',
						borderColor: 'outline-variant',
						overflowX: 'auto'
					},
					trigger: {
						textStyle: 'label-md',
						color: 'on-surface-variant',
						background: 'transparent',
						border: 'none',
						borderBottomWidth: '2px',
						borderBottomStyle: 'solid',
						borderBottomColor: 'transparent',
						marginBottom: '-1px',
						padding: '8px 4px',
						whiteSpace: 'nowrap',
						cursor: 'pointer',
						transitionProperty: 'color, border-color',
						transitionDuration: '150ms',
						_hover: { color: 'on-surface' },
						'&[aria-selected=true]': { color: 'primary', borderBottomColor: 'primary', fontWeight: 700 },
						_focusVisible: { outline: '2px solid {colors.primary}', outlineOffset: '-2px' }
					},
					content: {
						paddingTop: '24px',
						_focusVisible: { outline: 'none' }
					}
				}
			},
			checkbox: {
				className: 'checkbox',
				slots: ['root', 'control', 'indicator', 'label'],
				base: {
					root: { display: 'inline-flex', alignItems: 'center', gap: '10px', cursor: 'pointer' },
					control: {
						display: 'flex',
						alignItems: 'center',
						justifyContent: 'center',
						width: '18px',
						height: '18px',
						borderRadius: 'sm',
						borderWidth: '2px',
						borderStyle: 'solid',
						borderColor: 'outline',
						background: 'transparent',
						transitionProperty: 'background, border-color',
						transitionDuration: '150ms',
						flexShrink: 0,
						'&[data-state=checked], &[data-state=indeterminate]': {
							background: 'primary-container',
							borderColor: 'primary-container',
							color: 'on-primary-container'
						},
						_focusVisible: { outline: '2px solid {colors.primary}', outlineOffset: '2px' }
					},
					indicator: { display: 'flex', color: 'inherit', '& svg': { width: '14px', height: '14px' } },
					label: { textStyle: 'body-md', color: 'on-surface' }
				}
			},
			tooltip: {
				className: 'tooltip',
				slots: ['trigger', 'positioner', 'content', 'arrow'],
				base: {
					trigger: {
						display: 'inline-flex',
						alignItems: 'center',
						justifyContent: 'center',
						background: 'transparent',
						border: 'none',
						padding: '2px',
						borderRadius: 'sm',
						color: 'on-surface-variant',
						cursor: 'pointer',
						_hover: { color: 'primary' },
						_focusVisible: { outline: '2px solid {colors.primary}', outlineOffset: '1px' }
					},
					positioner: { zIndex: 60 },
					content: {
						background: 'surface-container-highest',
						color: 'on-surface',
						borderWidth: '1px',
						borderStyle: 'solid',
						borderColor: 'outline-variant',
						borderRadius: 'md',
						padding: '6px 10px',
						textStyle: 'body-sm',
						maxWidth: '280px',
						boxShadow: 'popup',
						animation: 'fade-in'
					},
					arrow: {}
				}
			},
			select: {
				className: 'select',
				slots: ['root', 'trigger', 'indicator', 'valueText', 'positioner', 'content', 'item', 'itemText', 'itemIndicator'],
				base: {
					root: { width: '100%', minWidth: '0' },
					trigger: {
						display: 'flex',
						alignItems: 'center',
						justifyContent: 'space-between',
						gap: '8px',
						width: '100%',
						minHeight: '44px',
						padding: '10px 12px',
						background: 'surface-container-high',
						color: 'on-surface',
						borderWidth: '1px',
						borderStyle: 'solid',
						borderColor: 'outline-variant',
						borderRadius: 'md',
						textStyle: 'body-md',
						cursor: 'pointer',
						transitionProperty: 'border-color, box-shadow',
						transitionDuration: '150ms',
						'&[data-state=open]': { borderColor: 'primary', boxShadow: '0 0 0 1px {colors.primary}' },
						_focusVisible: { outline: 'none', borderColor: 'primary', boxShadow: '0 0 0 1px {colors.primary}' },
						_disabled: { opacity: 0.6, cursor: 'not-allowed' },
						'& svg': { width: '20px', height: '20px', flexShrink: 0, color: 'on-surface-variant' }
					},
					indicator: { display: 'flex', color: 'on-surface-variant' },
					valueText: {
						overflow: 'hidden',
						textOverflow: 'ellipsis',
						whiteSpace: 'nowrap',
						textAlign: 'left',
						'&[data-placeholder-shown]': { color: 'on-surface-variant' }
					},
					positioner: { zIndex: 60 },
					content: {
						width: '100%',
						background: 'surface-container-high',
						borderWidth: '1px',
						borderStyle: 'solid',
						borderColor: 'outline-variant',
						borderRadius: 'lg',
						boxShadow: 'popup',
						padding: '4px',
						maxHeight: '288px',
						overflowY: 'auto',
						animation: 'fade-in',
						outline: 'none'
					},
					item: {
						display: 'flex',
						alignItems: 'center',
						justifyContent: 'space-between',
						gap: '8px',
						padding: '8px 12px',
						borderRadius: 'md',
						textStyle: 'body-md',
						color: 'on-surface',
						cursor: 'pointer',
						transitionProperty: 'background, color',
						transitionDuration: '100ms',
						'&[data-highlighted]': { background: 'surface-variant' },
						'&[data-disabled]': { opacity: 0.5, cursor: 'not-allowed' },
						_focusVisible: { outline: 'none' }
					},
					itemText: { overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' },
					itemIndicator: { display: 'flex', color: 'primary', '& svg': { width: '18px', height: '18px' } }
				}
			},
			toast: {
				className: 'toast',
				slots: ['group', 'root', 'title', 'description', 'closeTrigger'],
				base: {
					group: { pointerEvents: 'none' },
					root: {
						pointerEvents: 'auto',
						display: 'flex',
						alignItems: 'flex-start',
						gap: '12px',
						width: '360px',
						maxWidth: 'calc(100vw - 32px)',
						padding: '12px 16px',
						background: 'surface-container-highest',
						color: 'on-surface',
						borderWidth: '1px',
						borderStyle: 'solid',
						borderColor: 'outline-variant',
						borderRadius: 'lg',
						boxShadow: 'popup',
						animation: 'content-in',
						'&[data-type=error]': { borderColor: 'error/50', color: 'error' },
						'& svg': { width: '20px', height: '20px', flexShrink: 0 }
					},
					title: { textStyle: 'body-sm', fontWeight: 600 },
					description: { textStyle: 'body-sm', color: 'on-surface-variant' },
					closeTrigger: {
						marginLeft: 'auto',
						display: 'flex',
						background: 'transparent',
						border: 'none',
						padding: '2px',
						borderRadius: 'sm',
						color: 'on-surface-variant',
						cursor: 'pointer',
						_hover: { color: 'on-surface' }
					}
				}
			}
		}
	},
	globalCss: {
		':root': { colorScheme: 'light' },
		'.dark': { colorScheme: 'dark' },
		body: {
			background: 'background',
			color: 'on-surface',
			fontFamily: 'sans',
			textStyle: 'body-md',
			WebkitFontSmoothing: 'antialiased'
		},
		'::selection': { background: 'primary', color: 'on-primary' },
		'*': { borderColor: 'outline-variant' },
		'::-webkit-scrollbar': { width: '8px', height: '8px' },
		'::-webkit-scrollbar-track': { background: 'surface-container-low' },
		'::-webkit-scrollbar-thumb': { background: 'surface-container-highest', borderRadius: '4px' },
		'::-webkit-scrollbar-thumb:hover': { background: 'outline-variant' },
		'@media (prefers-reduced-motion: reduce)': {
			'*, *::before, *::after': {
				animationDuration: '0.01ms !important',
				animationIterationCount: '1 !important',
				transitionDuration: '0.01ms !important',
				scrollBehavior: 'auto !important'
			}
		}
	}
});
