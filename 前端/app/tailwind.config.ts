import type { Config } from "tailwindcss";

/**
 * Haven Design System - Tailwind CSS Configuration
 * 
 * Instructions:
 * Merge this configuration into your project's tailwind.config.ts.
 * This file maps the CSS variables defined in global-tokens.css into Tailwind utilities,
 * ensuring that developers use only the approved Design Tokens (e.g., `bg-primary`, `text-secondary`)
 * and preventing the use of magic numbers (e.g., `mt-[17px]`).
 */

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx,js,jsx}"],
  darkMode: ["class"],
  theme: {
    container: {
      center: true,
      padding: "2rem",
      screens: {
        "2xl": "1400px",
      },
    },
    extend: {
      fontFamily: {
        sans: [
          'Inter',
          '-apple-system',
          'BlinkMacSystemFont',
          '"SF Pro Text"',
          '"Segoe UI"',
          'Roboto',
          '"Helvetica Neue"',
          'Arial',
          'sans-serif'
        ],
      },
      colors: {
        // shadcn/ui mapped colors
        border: "var(--border)",
        input: "var(--input)",
        ring: "var(--ring)",
        background: "var(--background)",
        foreground: "var(--foreground)",
        primary: {
          DEFAULT: "var(--primary)",
          foreground: "var(--primary-foreground)",
          hover: "var(--ds-color-action-hover)",
          pressed: "var(--ds-color-action-pressed)",
        },
        secondary: {
          DEFAULT: "var(--secondary)",
          foreground: "var(--secondary-foreground)",
        },
        destructive: {
          DEFAULT: "var(--destructive)",
          foreground: "var(--destructive-foreground)",
        },
        muted: {
          DEFAULT: "var(--muted)",
          foreground: "var(--muted-foreground)",
        },
        accent: {
          DEFAULT: "var(--accent)",
          foreground: "var(--accent-foreground)",
        },
        popover: {
          DEFAULT: "var(--popover)",
          foreground: "var(--popover-foreground)",
        },
        card: {
          DEFAULT: "var(--card)",
          foreground: "var(--card-foreground)",
        },

        // Haven Specific Semantic Colors
        surface: {
          primary: "var(--ds-color-surface-primary)",
          secondary: "var(--ds-color-surface-secondary)",
          elevated: "var(--ds-color-surface-elevated)",
        },
        text: {
          primary: "var(--ds-color-text-primary)",
          secondary: "var(--ds-color-text-secondary)",
          tertiary: "var(--ds-color-text-tertiary)",
          placeholder: "var(--ds-color-text-placeholder)",
          disabled: "var(--ds-color-text-disabled)",
          inverse: "var(--ds-color-text-inverse)",
        }
      },
      spacing: {
        2: "var(--ds-space-2)",
        4: "var(--ds-space-4)",
        8: "var(--ds-space-8)",
        12: "var(--ds-space-12)",
        16: "var(--ds-space-16)",
        20: "var(--ds-space-20)",
        24: "var(--ds-space-24)",
        32: "var(--ds-space-32)",
        40: "var(--ds-space-40)",
        48: "var(--ds-space-48)",
        64: "var(--ds-space-64)",
        
        // Composition Tokens (Spacing/Padding/Gaps)
        'page-compact': "var(--ds-layout-page-padding-compact)",
        'page-regular': "var(--ds-layout-page-padding-regular)",
        'header-title': "var(--ds-layout-pageHeader-titleGap)",
        'header-section': "var(--ds-layout-pageHeader-sectionGap)",
        'grid-column': "var(--ds-layout-contentGrid-columnGap)",

        // Optical Tokens
        'icon-text': "var(--ds-optical-iconTextGap)",
        'cover-text': "var(--ds-optical-coverTextGap)",
        'title-meta': "var(--ds-optical-titleMetadataGap)",
        'nav-inset': "var(--ds-optical-navItemInset)",
      },
      width: {
        'sidebar-expanded': "var(--ds-layout-sidebar-expanded-width)",
        'sidebar-collapsed': "var(--ds-layout-sidebar-collapsed-width)",
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
        // Haven specific
        'ds-sm': "var(--ds-radius-sm)",
        'ds-md': "var(--ds-radius-md)",
        'ds-lg': "var(--ds-radius-lg)",
        'ds-xl': "var(--ds-radius-xl)",
        'ds-2xl': "var(--ds-radius-2xl)",
      },
      fontSize: {
        // Fluid typography mappings
        'ds-sm': "var(--ds-text-sm)",
        'ds-base': "var(--ds-text-base)",
        'ds-lg': "var(--ds-text-lg)",
        'ds-xl': "var(--ds-text-xl)",
        'ds-2xl': "var(--ds-text-2xl)",
        'ds-3xl': "var(--ds-text-3xl)",
      },
      boxShadow: {
        'ds-sm': "var(--ds-shadow-sm)",
        'ds-md': "var(--ds-shadow-md)",
        'ds-lg': "var(--ds-shadow-lg)",
      },
      transitionDuration: {
        fast: "var(--ds-motion-fast)",
        normal: "var(--ds-motion-normal)",
        moderate: "var(--ds-motion-moderate)",
      },
      keyframes: {
        "accordion-down": {
          from: { height: "0" },
          to: { height: "var(--radix-accordion-content-height)" },
        },
        "accordion-up": {
          from: { height: "var(--radix-accordion-content-height)" },
          to: { height: "0" },
        },
        "cloud-breathe": {
          "0%, 100%": { transform: "translateY(0px) scale(1)" },
          "50%": { transform: "translateY(-14px) scale(1.02)" },
        },
      },
      animation: {
        "accordion-down": "accordion-down 0.2s ease-out",
        "accordion-up": "accordion-up 0.2s ease-out",
        "cloud-breathe": "cloud-breathe 6.5s ease-in-out infinite",
      },
    },
  },
};
