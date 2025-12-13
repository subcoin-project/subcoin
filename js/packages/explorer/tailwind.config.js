/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        bitcoin: {
          orange: "#f7931a",
          dark: "#1a1a2e",
          darker: "#0f0f1a",
        },
      },
    },
  },
  plugins: [],
};
