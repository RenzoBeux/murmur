/** @type {import('tailwindcss').Config} */
module.exports = {
    darkMode: ['class'],
    content: [
    './src/pages/**/*.{js,ts,jsx,tsx,mdx}',
    './src/components/**/*.{js,ts,jsx,tsx,mdx}',
    './src/app/**/*.{js,ts,jsx,tsx,mdx}',
    // src/lib holds literal class strings for the palettes that are picked at
    // runtime (speakerLabel.ts, projectColors.ts). Without this glob those
    // utilities are purged and the chips render uncolored.
    './src/lib/**/*.{js,ts,jsx,tsx,mdx}',
  ],
  theme: {
  	extend: {
  		fontFamily: {
  			sans: [
  				'var(--font-inter)',
  				'system-ui',
  				'sans-serif'
  			],
  			mono: [
  				'var(--font-mono)',
  				'ui-monospace',
  				'monospace'
  			]
  		},
  		fontSize: {
  			display: ['1.75rem', { lineHeight: '2.25rem', fontWeight: '600', letterSpacing: '-0.02em' }],
  			h1: ['1.375rem', { lineHeight: '1.875rem', fontWeight: '600', letterSpacing: '-0.01em' }],
  			h2: ['1.125rem', { lineHeight: '1.625rem', fontWeight: '600' }],
  			h3: ['1rem', { lineHeight: '1.5rem', fontWeight: '600' }],
  			body: ['0.875rem', { lineHeight: '1.375rem' }],
  			small: ['0.8125rem', { lineHeight: '1.125rem' }],
  			caption: ['0.75rem', { lineHeight: '1rem', letterSpacing: '0.02em' }]
  		},
  		colors: {
  			background: 'hsl(var(--background) / <alpha-value>)',
  			foreground: 'hsl(var(--foreground) / <alpha-value>)',
  			border: 'hsl(var(--border) / <alpha-value>)',
  			input: 'hsl(var(--input) / <alpha-value>)',
  			ring: 'hsl(var(--ring) / <alpha-value>)',
  			primary: {
  				DEFAULT: 'hsl(var(--primary) / <alpha-value>)',
  				foreground: 'hsl(var(--primary-foreground) / <alpha-value>)'
  			},
  			secondary: {
  				DEFAULT: 'hsl(var(--secondary) / <alpha-value>)',
  				foreground: 'hsl(var(--secondary-foreground) / <alpha-value>)'
  			},
  			card: {
  				DEFAULT: 'hsl(var(--card) / <alpha-value>)',
  				foreground: 'hsl(var(--card-foreground) / <alpha-value>)'
  			},
  			popover: {
  				DEFAULT: 'hsl(var(--popover) / <alpha-value>)',
  				foreground: 'hsl(var(--popover-foreground) / <alpha-value>)'
  			},
  			muted: {
  				DEFAULT: 'hsl(var(--muted) / <alpha-value>)',
  				foreground: 'hsl(var(--muted-foreground) / <alpha-value>)'
  			},
  			accent: {
  				DEFAULT: 'hsl(var(--accent) / <alpha-value>)',
  				foreground: 'hsl(var(--accent-foreground) / <alpha-value>)'
  			},
  			destructive: {
  				DEFAULT: 'hsl(var(--destructive) / <alpha-value>)',
  				foreground: 'hsl(var(--destructive-foreground) / <alpha-value>)'
  			},
  			brand: {
  				DEFAULT: 'hsl(var(--brand) / <alpha-value>)',
  				hover: 'hsl(var(--brand-hover) / <alpha-value>)',
  				active: 'hsl(var(--brand-active) / <alpha-value>)',
  				foreground: 'hsl(var(--brand-foreground) / <alpha-value>)'
  			},
  			elevated: 'hsl(var(--elevated) / <alpha-value>)',
  			overlay: 'hsl(var(--overlay) / <alpha-value>)',
  			success: {
  				DEFAULT: 'hsl(var(--success) / <alpha-value>)',
  				foreground: 'hsl(var(--success-foreground) / <alpha-value>)'
  			},
  			warning: {
  				DEFAULT: 'hsl(var(--warning) / <alpha-value>)',
  				foreground: 'hsl(var(--warning-foreground) / <alpha-value>)'
  			},
  			speaker: {
  				you: 'hsl(var(--speaker-you) / <alpha-value>)',
  				others: 'hsl(var(--speaker-others) / <alpha-value>)',
  				'1': 'hsl(var(--speaker-1) / <alpha-value>)',
  				'2': 'hsl(var(--speaker-2) / <alpha-value>)',
  				'3': 'hsl(var(--speaker-3) / <alpha-value>)',
  				'4': 'hsl(var(--speaker-4) / <alpha-value>)',
  				'5': 'hsl(var(--speaker-5) / <alpha-value>)',
  				'6': 'hsl(var(--speaker-6) / <alpha-value>)'
  			},
  			project: {
  				violet: 'hsl(var(--project-violet) / <alpha-value>)',
  				blue: 'hsl(var(--project-blue) / <alpha-value>)',
  				cyan: 'hsl(var(--project-cyan) / <alpha-value>)',
  				teal: 'hsl(var(--project-teal) / <alpha-value>)',
  				green: 'hsl(var(--project-green) / <alpha-value>)',
  				amber: 'hsl(var(--project-amber) / <alpha-value>)',
  				orange: 'hsl(var(--project-orange) / <alpha-value>)',
  				rose: 'hsl(var(--project-rose) / <alpha-value>)'
  			},
  			chart: {
  				'1': 'hsl(var(--chart-1) / <alpha-value>)',
  				'2': 'hsl(var(--chart-2) / <alpha-value>)',
  				'3': 'hsl(var(--chart-3) / <alpha-value>)',
  				'4': 'hsl(var(--chart-4) / <alpha-value>)',
  				'5': 'hsl(var(--chart-5) / <alpha-value>)'
  			}
  		},
  		boxShadow: {
  			glow: '0 0 0 1px hsl(var(--brand) / 0.25), 0 0 24px -6px hsl(var(--brand) / 0.35)',
  			'glow-destructive': '0 0 0 1px hsl(var(--destructive) / 0.3), 0 0 24px -6px hsl(var(--destructive) / 0.4)',
  			glass: 'inset 0 1px 0 0 hsl(0 0% 100% / 0.06), 0 8px 32px -12px hsl(var(--overlay) / 0.6)'
  		},
  		borderRadius: {
  			lg: 'var(--radius)',
  			md: 'calc(var(--radius) - 2px)',
  			sm: 'calc(var(--radius) - 4px)'
  		},
  		keyframes: {
  			'accordion-down': {
  				from: {
  					height: '0'
  				},
  				to: {
  					height: 'var(--radix-accordion-content-height)'
  				}
  			},
  			'accordion-up': {
  				from: {
  					height: 'var(--radix-accordion-content-height)'
  				},
  				to: {
  					height: '0'
  				}
  			}
  		},
  		animation: {
  			'accordion-down': 'accordion-down 0.2s ease-out',
  			'accordion-up': 'accordion-up 0.2s ease-out'
  		}
  	}
  },
  plugins: [
    require("tailwindcss-animate"),
    require("@tailwindcss/typography"),
    require("@tailwindcss/container-queries"),
  ],
}
