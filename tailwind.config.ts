import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./overlay.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      spacing: {
        "42": "10.5rem",
      },
      colors: {
        accent: {
          DEFAULT: "#00d5bd",
          hover: "#00bfa9",
        },
        hud: {
          bg: "#05070a",
          panel: "#0a0e17",
          content: "#070a0f",
          cardHover: "#151d30",
          navActive: "#1b2436",
          border: "#1a243a",
          placeholder: "#3b485e",
        },
      },
      boxShadow: {
        "accent-button": "0 0 8px rgba(0, 213, 189, 0.15)",
        "accent-dot": "0 0 4px #00d5bd",
        "modal-overlay": "0 0 24px rgba(0, 0, 0, 0.45)",
      },
    },
  },
} satisfies Config;
