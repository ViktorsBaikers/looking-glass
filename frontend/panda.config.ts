import { defineConfig } from '@pandacss/dev';
import { pluginSvelte } from '@pandacss/plugin-svelte';

// "Backbone map" world: see DESIGN.md. Every semantic colour is light-dark(), so
// theme follows color-scheme: `:root`/`.light` light, `.dark` (the class the
// pre-paint script in app.html toggles) dark. No `_dark` conditions.
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
	// Hover styles only where a real pointer hovers: on touch, a tap would
	// otherwise leave the hover colour stuck on until the next tap elsewhere.
	conditions: {
		extend: {
			hover: ['@media (hover: hover) and (pointer: fine)', '&:is(:hover, [data-hover])']
		}
	},
	outdir: 'styled-system',
	theme: {
		keyframes: {
			spin: { to: { transform: 'rotate(360deg)' } },
			'fade-in': { from: { opacity: 0 }, to: { opacity: 1 } },
			'fade-out': { from: { opacity: 1 }, to: { opacity: 0 } },
			// Popovers and dialogs grow from 96% of their size, never from nothing.
			'pop-in': {
				from: { opacity: 0, transform: 'scale(0.96)' },
				to: { opacity: 1, transform: 'scale(1)' }
			},
			'pop-out': {
				from: { opacity: 1, transform: 'scale(1)' },
				to: { opacity: 0, transform: 'scale(0.96)' }
			},
			'drawer-in': { from: { transform: 'translateX(-100%)' }, to: { transform: 'translateX(0)' } },
			'drawer-out': { from: { transform: 'translateX(0)' }, to: { transform: 'translateX(-100%)' } },
			// Copy confirmation: the check arrives with a little weight.
			'check-in': {
				from: { opacity: 0, transform: 'scale(0.6)' },
				to: { opacity: 1, transform: 'scale(1)' }
			},
			// A station arriving on the route: the marker drops in, the row fades up.
			'station-in': {
				from: { opacity: 0, transform: 'translateY(-4px)' },
				to: { opacity: 1, transform: 'translateY(0)' }
			},
			// The live "you are here" marker on the newest station: a ring leaves
			// the marker and fades (transform + opacity only, so no repaint).
			'here-pulse': {
				'0%': { transform: 'scale(1)', opacity: 0.9 },
				'60%': { opacity: 0.35 },
				'100%': { transform: 'scale(2)', opacity: 0 }
			}
		},
		tokens: {
			fonts: {
				sans: { value: ["'Overpass Variable'", "'Overpass'", 'system-ui', 'sans-serif'] },
				mono: { value: ["'Overpass Mono Variable'", "'Overpass Mono'", 'ui-monospace', 'monospace'] }
			},
			// Map grammar: panels are square, controls barely softened, roundels round.
			radii: {
				none: { value: '0' },
				sm: { value: '2px' },
				md: { value: '3px' },
				full: { value: '9999px' }
			},
			shadows: {
				popup: {
					value: '0 1px 2px rgba(10, 11, 13, 0.08), 0 12px 32px -8px rgba(10, 11, 13, 0.28)'
				}
			},
			easings: {
				out: { value: 'cubic-bezier(0.16, 1, 0.3, 1)' },
				drawer: { value: 'cubic-bezier(0.32, 0.72, 0, 1)' },
				'in-out': { value: 'cubic-bezier(0.77, 0, 0.175, 1)' }
			},
			animations: {
				spin: { value: 'spin 1s linear infinite' },
				'fade-in': { value: 'fade-in 160ms cubic-bezier(0.16, 1, 0.3, 1)' },
				'fade-out': { value: 'fade-out 120ms cubic-bezier(0.16, 1, 0.3, 1) forwards' },
				'pop-in': { value: 'pop-in 180ms cubic-bezier(0.16, 1, 0.3, 1)' },
				'pop-out': { value: 'pop-out 140ms cubic-bezier(0.16, 1, 0.3, 1) forwards' },
				'drawer-in': { value: 'drawer-in 300ms cubic-bezier(0.32, 0.72, 0, 1)' },
				'drawer-out': { value: 'drawer-out 200ms cubic-bezier(0.32, 0.72, 0, 1) forwards' },
				'check-in': { value: 'check-in 200ms cubic-bezier(0.16, 1, 0.3, 1)' },
				'station-in': { value: 'station-in 240ms cubic-bezier(0.16, 1, 0.3, 1) both' },
				'here-pulse': { value: 'here-pulse 1.6s cubic-bezier(0.25, 0.1, 0.25, 1) infinite' }
			}
		},
		// Light = the printed map (neutral paper, ink). Dark = the night map
		// (charcoal, not navy). Colour lives only on data: Location lines, stations,
		// and the status signals below. Line colours are assigned in lib/lines.ts.
		// Every value is light-dark(), so any subtree flips theme through
		// color-scheme alone (`.dark` / `.light`, e.g. the settings previews).
		semanticTokens: {
			colors: {
				paper: { value: 'light-dark(#f3f3f0, #111214)' },
				panel: { value: 'light-dark(#ffffff, #18191c)' },
				sunk: { value: 'light-dark(#ebebe7, #0b0c0d)' },
				ink: { value: 'light-dark(#16171a, #ecece6)' },
				'ink-hover': { value: 'light-dark(#33353b, #ffffff)' },
				'ink-muted': { value: 'light-dark(#595c63, #a2a5ab)' },
				'ink-faint': { value: 'light-dark(#8b8e94, #6c6f76)' },
				'on-ink': { value: 'light-dark(#ffffff, #111214)' },
				rule: { value: 'light-dark(#dcdcd6, #2a2c30)' },
				'rule-strong': { value: 'light-dark(#8a8c87, #686b72)' },
				ok: { value: 'light-dark(#1d7a4a, #52c98b)' },
				'ok-soft': { value: 'light-dark(#e3f1e8, #15291e)' },
				warn: { value: 'light-dark(#9a5200, #f5b453)' },
				'warn-soft': { value: 'light-dark(#f8ecd9, #2e2312)' },
				danger: { value: 'light-dark(#b3261e, #ff8a80)' },
				'danger-soft': { value: 'light-dark(#f9e3e1, #331a19)' }
			}
		},
		// One strict scale: 12 · 13 · 14 · 16 · 20 · 28 · 40. Every role below sits on it.
		textStyles: {
			display: {
				value: {
					fontSize: '40px',
					lineHeight: '44px',
					fontWeight: 800,
					letterSpacing: '-0.03em'
				}
			},
			'display-sm': {
				value: { fontSize: '28px', lineHeight: '32px', fontWeight: 800, letterSpacing: '-0.025em' }
			},
			title: {
				value: { fontSize: '20px', lineHeight: '26px', fontWeight: 700, letterSpacing: '-0.015em' }
			},
			body: { value: { fontSize: '16px', lineHeight: '24px', fontWeight: 400 } },
			'body-sm': { value: { fontSize: '14px', lineHeight: '20px', fontWeight: 400 } },
			label: { value: { fontSize: '13px', lineHeight: '16px', fontWeight: 600, letterSpacing: '0' } },
			caption: { value: { fontSize: '12px', lineHeight: '16px', fontWeight: 500 } },
			// The name half of a fact: fact keys, column heads, nav group heads, tags.
			key: { value: { fontSize: '12px', lineHeight: '16px', fontWeight: 700 } },
			// A named thing inside a list or card: row names, card heads, the brand.
			item: { value: { fontSize: '16px', lineHeight: '22px', fontWeight: 800, letterSpacing: '-0.01em' } },
			// A fact's value in sans (status, place, facility). Not `value`: Panda reads that key as a leaf.
			fact: { value: { fontSize: '16px', lineHeight: '22px', fontWeight: 600 } },
			code: {
				value: {
					fontFamily: 'mono',
					fontSize: '13px',
					lineHeight: '20px',
					fontWeight: 400,
					fontVariantNumeric: 'tabular-nums'
				}
			},
			// Inline data outside the console: addresses, ASNs, commands.
			data: {
				value: {
					fontFamily: 'mono',
					fontSize: '14px',
					lineHeight: '22px',
					fontWeight: 400,
					fontVariantNumeric: 'tabular-nums'
				}
			},
			numeral: {
				value: {
					fontSize: '28px',
					lineHeight: '32px',
					fontWeight: 700,
					letterSpacing: '-0.02em',
					fontVariantNumeric: 'tabular-nums'
				}
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
					borderRadius: 'sm',
					fontFamily: 'sans',
					fontSize: '14px',
					lineHeight: '16px',
					fontWeight: 700,
					cursor: 'pointer',
					transitionProperty: 'background, color, border-color, transform',
					transitionDuration: '140ms',
					transitionTimingFunction: 'out',
					// Press feedback on every variant: the button gives a little.
					_active: { transform: 'scale(0.97)' },
					borderStyle: 'solid',
					borderWidth: '1px',
					borderColor: 'transparent',
					textDecoration: 'none',
					_disabled: { opacity: 0.45, cursor: 'not-allowed', pointerEvents: 'none' },
					_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' },
					'& svg': { width: '18px', height: '18px', flexShrink: 0 }
				},
				variants: {
					variant: {
						primary: {
							background: 'ink',
							color: 'on-ink',
							_hover: { background: 'ink-hover' }
						},
						secondary: {
							background: 'panel',
							color: 'ink',
							borderColor: 'rule-strong',
							_hover: { borderColor: 'ink' }
						},
						ghost: {
							background: 'transparent',
							color: 'ink-muted',
							_hover: { color: 'ink', background: 'sunk' }
						},
						danger: {
							background: 'transparent',
							color: 'danger',
							_hover: { background: 'danger-soft' }
						}
					},
					size: {
						sm: { minHeight: '32px', padding: '0 12px', fontSize: '13px' },
						md: { minHeight: '40px', padding: '0 16px' },
						lg: { minHeight: '48px', padding: '0 24px', fontSize: '16px' },
						icon: { minHeight: '36px', minWidth: '36px', padding: '0' }
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
					borderRadius: 'sm',
					padding: '2px 6px',
					textStyle: 'key',
					whiteSpace: 'nowrap'
				},
				variants: {
					tone: {
						neutral: { background: 'sunk', color: 'ink-muted' },
						success: { background: 'ok-soft', color: 'ok' },
						warning: { background: 'warn-soft', color: 'warn' },
						danger: { background: 'danger-soft', color: 'danger' }
					},
					// `lg`: the Location status signal, no fill, the line segment carries it.
					size: {
						sm: {},
						lg: { background: 'transparent', padding: '0', gap: '8px', textStyle: 'label' }
					}
				},
				compoundVariants: [
					{ size: 'lg', tone: ['success', 'warning', 'danger'], css: { color: 'ink' } },
					{ size: 'lg', tone: 'neutral', css: { color: 'ink-muted' } }
				],
				defaultVariants: { tone: 'neutral', size: 'sm' }
			},
			input: {
				className: 'input',
				base: {
					width: '100%',
					minHeight: '40px',
					background: 'panel',
					color: 'ink',
					borderWidth: '1px',
					borderStyle: 'solid',
					borderColor: 'rule-strong',
					borderRadius: 'sm',
					padding: '8px 12px',
					fontFamily: 'sans',
					// 16px: the body floor, and iOS zooms the page on focus below it.
					fontSize: '16px',
					lineHeight: '22px',
					transitionProperty: 'border-color, box-shadow',
					transitionDuration: '120ms',
					caretColor: '{colors.ink}',
					// ink-muted: placeholders carry format examples, so they need 4.5:1.
					_placeholder: { color: 'ink-muted' },
					_hover: { borderColor: 'ink-muted' },
					_focus: {
						outline: 'none',
						borderColor: 'ink',
						boxShadow: 'inset 0 0 0 1px {colors.ink}'
					},
					_disabled: { opacity: 0.55, cursor: 'not-allowed', background: 'sunk' }
				},
				variants: {
					invalid: {
						true: {
							borderColor: 'danger',
							_hover: { borderColor: 'danger' },
							_focus: { borderColor: 'danger', boxShadow: 'inset 0 0 0 1px {colors.danger}' }
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
					padding: '12px 14px',
					borderRadius: 'sm',
					borderWidth: '1px',
					borderStyle: 'solid',
					borderColor: 'rule',
					background: 'panel',
					cursor: 'pointer',
					transitionProperty: 'border-color, background',
					transitionDuration: '120ms',
					_hover: { borderColor: 'ink-muted' },
					'&:has([data-state=checked])': { borderColor: 'ink' },
					'&[data-disabled], &:has([data-disabled])': { opacity: 0.55, cursor: 'not-allowed' }
				}
			}
		},
		slotRecipes: {
			field: {
				className: 'field',
				slots: ['root', 'label', 'control', 'error', 'hint'],
				base: {
					root: { display: 'flex', flexDirection: 'column', gap: '6px', minWidth: '0' },
					label: { textStyle: 'label', color: 'ink' },
					control: {},
					error: { textStyle: 'body-sm', color: 'danger' },
					hint: { textStyle: 'body-sm', color: 'ink-muted' }
				}
			},
			card: {
				className: 'card',
				slots: ['root', 'header', 'title', 'description', 'content', 'footer'],
				base: {
					root: {
						background: 'panel',
						color: 'ink',
						borderWidth: '1px',
						borderStyle: 'solid',
						borderColor: 'rule',
						borderRadius: 'none',
						padding: '24px'
					},
					header: { display: 'flex', flexDirection: 'column', gap: '4px', marginBottom: '20px' },
					title: { textStyle: 'title', color: 'ink' },
					description: { textStyle: 'body-sm', color: 'ink-muted' },
					content: {},
					footer: { display: 'flex', alignItems: 'center', gap: '12px', marginTop: '24px' }
				}
			},
			dialog: {
				className: 'dialog',
				slots: ['backdrop', 'positioner', 'content', 'title', 'description', 'closeTrigger'],
				base: {
					backdrop: {
						position: 'fixed',
						inset: '0',
						background: 'rgba(10, 11, 13, 0.55)',
						zIndex: 50,
						_open: { animation: 'fade-in' },
						_closed: { animation: 'fade-out' }
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
						background: 'panel',
						color: 'ink',
						borderTopWidth: '4px',
						borderTopStyle: 'solid',
						borderTopColor: 'ink',
						borderRadius: 'none',
						padding: '28px',
						width: '100%',
						maxWidth: '520px',
						maxHeight: '90vh',
						overflowY: 'auto',
						boxShadow: 'popup',
						outline: 'none',
						// A modal is not anchored to its trigger: it grows from its centre.
						_open: { animation: 'pop-in' },
						_closed: { animation: 'pop-out' }
					},
					title: { textStyle: 'title', marginBottom: '6px', paddingRight: '32px' },
					description: { textStyle: 'body-sm', color: 'ink-muted', marginBottom: '20px' },
					closeTrigger: {
						position: 'absolute',
						top: '16px',
						right: '16px',
						display: 'flex',
						padding: '6px',
						borderRadius: 'sm',
						color: 'ink-muted',
						cursor: 'pointer',
						background: 'transparent',
						border: 'none',
						_hover: { color: 'ink', background: 'sunk' },
						_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '1px' },
						'& svg': { width: '18px', height: '18px' }
					}
				}
			},
			// Two grammars. `stops`: sequential sections drawn as stops on one line
			// (location editor). `routes`: Location lines, each led by its roundel.
			tabs: {
				className: 'tabs',
				slots: ['root', 'list', 'trigger', 'meta', 'content'],
				base: {
					root: { minWidth: '0' },
					list: { display: 'flex', overflowX: 'auto', scrollbarWidth: 'thin' },
					trigger: {
						position: 'relative',
						display: 'inline-flex',
						alignItems: 'center',
						background: 'transparent',
						border: 'none',
						whiteSpace: 'nowrap',
						cursor: 'pointer',
						color: 'ink-muted',
						fontFamily: 'sans',
						transitionProperty: 'color',
						transitionDuration: '120ms',
						_hover: { color: 'ink' },
						'&[aria-selected=true]': { color: 'ink' },
						_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '-2px' }
					},
					meta: { fontFamily: 'mono', fontSize: '13px', fontWeight: 400, color: 'ink-muted' },
					content: { _focusVisible: { outline: 'none' } }
				},
				variants: {
					variant: {
						stops: {
							list: {
								position: 'relative',
								gap: '0',
								paddingBottom: '2px',
								// The line the stops sit on, drawn through every marker's centre.
								_before: {
									content: '""',
									position: 'absolute',
									left: '6px',
									right: '6px',
									top: '6px',
									height: '3px',
									background: 'rule',
									pointerEvents: 'none'
								}
							},
							trigger: {
								flexDirection: 'column',
								alignItems: 'flex-start',
								gap: '10px',
								padding: '0 28px 10px 0',
								fontSize: '14px',
								lineHeight: '16px',
								fontWeight: 600,
								_before: {
									content: '""',
									width: '15px',
									height: '15px',
									borderRadius: 'full',
									background: 'paper',
									borderWidth: '3px',
									borderStyle: 'solid',
									borderColor: 'rule-strong',
									position: 'relative',
									zIndex: 1,
									transitionProperty: 'background, border-color',
									transitionDuration: '160ms'
								},
								_hover: { _before: { borderColor: 'ink' } },
								'&[aria-selected=true]': {
									fontWeight: 800,
									// Forced colours repaint the ink fill like the other stops.
									_before: {
										background: 'ink',
										borderColor: 'ink',
										_highContrast: {
											forcedColorAdjust: 'none',
											background: 'Highlight',
											borderColor: 'Highlight'
										}
									}
								}
							},
							content: { paddingTop: '28px' }
						},
						routes: {
							list: {
								gap: '4px',
								borderBottomWidth: '1px',
								borderBottomStyle: 'solid',
								borderBottomColor: 'rule'
							},
							trigger: {
								gap: '10px',
								padding: '10px 14px 12px 4px',
								marginBottom: '-1px',
								fontSize: '16px',
								lineHeight: '20px',
								fontWeight: 700,
								borderBottomWidth: '4px',
								borderBottomStyle: 'solid',
								borderBottomColor: 'transparent',
								// Forced colours paint a transparent border CanvasText, so every
								// tab would look selected: name the colours explicitly there.
								_highContrast: { borderBottomColor: 'Canvas' },
								transitionProperty: 'color, border-color',
								// The Location roundel: its code set in the line colour. The alt
								// text after "/" keeps the code out of the accessible name.
								// Unselected it is a ring in the line colour around a code in the
								// label's ink (a dimmed disc read below 4.5:1, F-261); the selected
								// and hovered roundel fills.
								_before: {
									content: 'attr(data-code) / ""',
									display: 'inline-flex',
									alignItems: 'center',
									justifyContent: 'center',
									width: '30px',
									height: '30px',
									borderRadius: 'full',
									borderWidth: '2px',
									borderStyle: 'solid',
									borderColor: 'var(--line)',
									background: 'transparent',
									color: 'ink-muted',
									fontSize: '10px',
									fontWeight: 800,
									letterSpacing: '0.02em',
									flexShrink: 0,
									transitionProperty: 'background, color',
									transitionDuration: '160ms'
								},
								_hover: { _before: { background: 'var(--line)', color: 'var(--line-ink)' } },
								'&[aria-selected=true]': {
									borderBottomColor: 'var(--line)',
									_highContrast: { borderBottomColor: 'Highlight' },
									_before: { background: 'var(--line)', color: 'var(--line-ink)' }
								}
							},
							content: { paddingTop: '28px' }
						}
					}
				},
				defaultVariants: { variant: 'stops' }
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
						borderColor: 'rule-strong',
						background: 'panel',
						transitionProperty: 'background, border-color',
						transitionDuration: '120ms',
						flexShrink: 0,
						'&[data-state=checked], &[data-state=indeterminate]': {
							background: 'ink',
							borderColor: 'ink',
							color: 'on-ink'
						},
						_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '2px' }
					},
					indicator: { display: 'flex', color: 'inherit', '& svg': { width: '14px', height: '14px' } },
					label: { textStyle: 'body', color: 'ink' }
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
						borderRadius: 'full',
						color: 'ink-faint',
						cursor: 'pointer',
						_hover: { color: 'ink' },
						_focusVisible: { outline: '2px solid {colors.ink}', outlineOffset: '1px' }
					},
					positioner: { zIndex: 60 },
					content: {
						background: 'ink',
						color: 'on-ink',
						borderRadius: 'sm',
						padding: '8px 10px',
						textStyle: 'body-sm',
						maxWidth: '280px',
						boxShadow: 'popup',
						transformOrigin: 'var(--transform-origin)',
						_open: { animation: 'pop-in 130ms cubic-bezier(0.16, 1, 0.3, 1)' },
						_closed: { animation: 'fade-out' },
						// Moving between tooltips while one is open: no delay, no motion.
						'&[data-instant]': { animation: 'none' }
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
						minHeight: '40px',
						padding: '8px 10px 8px 12px',
						background: 'panel',
						color: 'ink',
						borderWidth: '1px',
						borderStyle: 'solid',
						borderColor: 'rule-strong',
						borderRadius: 'sm',
						fontFamily: 'sans',
						fontSize: '16px',
						lineHeight: '22px',
						cursor: 'pointer',
						transitionProperty: 'border-color, box-shadow',
						transitionDuration: '120ms',
						_hover: { borderColor: 'ink-muted' },
						'&[data-state=open]': { borderColor: 'ink', boxShadow: 'inset 0 0 0 1px {colors.ink}' },
						_focusVisible: { outline: 'none', borderColor: 'ink', boxShadow: 'inset 0 0 0 1px {colors.ink}' },
						_disabled: { opacity: 0.55, cursor: 'not-allowed', background: 'sunk' },
						'& svg': { width: '20px', height: '20px', flexShrink: 0, color: 'ink-muted' }
					},
					indicator: { display: 'flex', color: 'ink-muted' },
					valueText: {
						overflow: 'hidden',
						textOverflow: 'ellipsis',
						whiteSpace: 'nowrap',
						textAlign: 'left',
						'&[data-placeholder-shown]': { color: 'ink-muted' }
					},
					positioner: { zIndex: 60 },
					content: {
						width: '100%',
						background: 'panel',
						borderWidth: '1px',
						borderStyle: 'solid',
						borderColor: 'ink',
						borderRadius: 'sm',
						boxShadow: 'popup',
						padding: '4px 0',
						maxHeight: '288px',
						overflowY: 'auto',
						outline: 'none',
						// Grows out of the trigger it belongs to.
						transformOrigin: 'var(--transform-origin)',
						_open: { animation: 'pop-in 160ms cubic-bezier(0.16, 1, 0.3, 1)' },
						_closed: { animation: 'fade-out' }
					},
					item: {
						display: 'flex',
						alignItems: 'center',
						justifyContent: 'space-between',
						gap: '8px',
						padding: '8px 12px',
						fontSize: '16px',
						lineHeight: '22px',
						color: 'ink',
						cursor: 'pointer',
						'&[data-highlighted]': {
							background: 'sunk',
							// Opt out of the forced repaint so no Canvas backplate hides the
							// HighlightText label; the check mark follows the label.
							_highContrast: {
								forcedColorAdjust: 'none',
								background: 'Highlight',
								color: 'HighlightText',
								'& svg': { color: 'HighlightText' }
							}
						},
						'&[data-state=checked]': { fontWeight: 700 },
						'&[data-disabled]': { opacity: 0.5, cursor: 'not-allowed' },
						_focusVisible: { outline: 'none' }
					},
					itemText: { overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' },
					itemIndicator: { display: 'flex', color: 'ink', '& svg': { width: '18px', height: '18px' } }
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
						padding: '12px 14px',
						background: 'ink',
						color: 'on-ink',
						borderRadius: 'sm',
						boxShadow: 'popup',
						// Ark positions every toast absolutely and drives the stack through
						// these variables; transitions (not keyframes) so a toast added
						// mid-animation retargets instead of restarting.
						translate: 'var(--x) var(--y)',
						zIndex: 'var(--z-index)',
						height: 'var(--height)',
						opacity: 'var(--opacity)',
						willChange: 'translate, opacity',
						transitionProperty: 'translate, opacity, height',
						transitionDuration: '360ms',
						transitionTimingFunction: 'out',
						_closed: { transitionDuration: '200ms' },
						'&[data-type=error]': { background: 'danger', color: 'on-ink' },
						'& svg': { width: '20px', height: '20px', flexShrink: 0 }
					},
					title: { textStyle: 'body-sm', fontWeight: 700 },
					description: { textStyle: 'body-sm', opacity: 0.85 },
					closeTrigger: {
						marginLeft: 'auto',
						display: 'flex',
						background: 'transparent',
						border: 'none',
						padding: '2px',
						borderRadius: 'sm',
						color: 'inherit',
						opacity: 0.7,
						cursor: 'pointer',
						_hover: { opacity: 1 }
					}
				}
			}
		}
	},
	globalCss: {
		':root, .light': { colorScheme: 'light' },
		'.dark': { colorScheme: 'dark' },
		html: { background: 'paper', scrollbarColor: '{colors.rule-strong} transparent' },
		// A sticky bottom bar (Settings' Save row) would cover fields focused
		// beneath it; focus scrolling keeps them above it.
		'html:has([data-sticky-actions])': { scrollPaddingBottom: '88px' },
		// Likewise the sticky site header (56px + 1px rule) at the top; auth
		// pages have no header.
		'html:has(header)': { scrollPaddingTop: '65px' },
		body: {
			background: 'paper',
			color: 'ink',
			fontFamily: 'sans',
			textStyle: 'body',
			WebkitFontSmoothing: 'antialiased',
			MozOsxFontSmoothing: 'grayscale',
			textRendering: 'optimizeLegibility',
			fontFeatureSettings: '"kern", "calt"'
		},
		'::selection': { background: 'ink', color: 'on-ink' },
		// Location lines (lib/lines.ts): the element declares both themes' colours.
		'[data-line]': {
			'--line': 'light-dark(var(--line-light), var(--line-dark))',
			'--line-ink': 'light-dark(var(--line-ink-light), #111214)'
		},
		'*': { borderColor: 'rule' },
		'a': { textUnderlineOffset: '3px', textDecorationThickness: '1px' },
		'input, textarea': { caretColor: '{colors.ink}' },
		'h1, h2, h3': { textWrap: 'balance' },
		p: { textWrap: 'pretty' },
		'*:focus-visible': { outlineColor: '{colors.ink}' },
		'::-webkit-scrollbar': { width: '10px', height: '10px' },
		'::-webkit-scrollbar-track': { background: 'transparent' },
		'::-webkit-scrollbar-thumb': {
			background: 'rule-strong',
			borderRadius: 'full',
			border: '3px solid transparent',
			backgroundClip: 'padding-box'
		},
		'::-webkit-scrollbar-thumb:hover': { background: 'ink-muted', backgroundClip: 'padding-box' },
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
