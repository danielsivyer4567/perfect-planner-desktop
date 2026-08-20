/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        background: "#0d1117",
        surface: "#161b22",
        surfaceHover: "#21262d",
        border: "#30363d",
        accent: "#38bdf8",
        brand: "#6366f1"
      }
    },
  },
  plugins: [],
}
