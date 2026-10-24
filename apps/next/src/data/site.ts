/**
 * Site-wide configuration — single source of truth.
 */

export const siteConfig = {
  name: "DLMan",
  tagline: "A fast, reliable, open-source download manager.",
  description:
    "Free and open-source download manager built with Rust. Multi-segment parallel downloads, crash-safe SQLite persistence, cross-platform. The modern alternative to IDM.",
  url: process.env.NEXT_PUBLIC_SITE_URL || "http://localhost:3000",
  github: {
    repo: "dilukshann7/dlman",
    url: "https://github.com/dilukshann7/dlman",
    releases: "https://github.com/dilukshann7/dlman/releases/latest",
    issues: "https://github.com/dilukshann7/dlman/issues",
    stars: "https://github.com/dilukshann7/dlman/stargazers",
    sponsor: "https://github.com/dilukshann7/dlman",
  },
  support: {
    buymeacoffee: "https://github.com/dilukshann7/dlman",
    githubSponsor: "https://github.com/dilukshann7/dlman",
  },
  firefox: "https://github.com/dilukshann7/dlman/releases/latest",
  builtBy: {
    name: "Dilukshan",
    url: "https://github.com/dilukshann7",
  },
  license: "MIT",
  currentYear: new Date().getFullYear(),
} as const;

export const navLinks = [
  { label: "Features", href: "/#features" },
  { label: "Download", href: "/download" },
  { label: "Docs", href: "/docs" },
  { label: "About", href: "/about" },
  { label: "Support", href: "/support" },
] as const;

export const footerLinks = {
  product: [
    { label: "Home", href: "/" },
    { label: "Download", href: "/download" },
    { label: "Features", href: "/#features" },
    { label: "About", href: "/about" },
  ],
  docs: [
    { label: "Introduction", href: "/docs/introduction" },
    { label: "Getting Started", href: "/docs" },
    { label: "CLI", href: "/docs/cli" },
    { label: "Browser Extension", href: "/docs/extension" },
    { label: "Core Engine", href: "/docs/core-engine" },
    { label: "Architecture", href: "/docs/architecture" },
    { label: "Contributing", href: "/docs/contributing" },
  ],
  github: [
    { label: "Repository", href: "https://github.com/dilukshann7/dlman" },
    { label: "Releases", href: "https://github.com/dilukshann7/dlman/releases" },
    { label: "Issues", href: "https://github.com/dilukshann7/dlman/issues" },
    { label: "Contributing", href: "/docs/contributing" },
  ],
  community: [
    { label: "Support", href: "/support" },
    { label: "GitHub Sponsors", href: "https://github.com/dilukshann7/dlman" },
    { label: "Buy Me a Coffee", href: "https://github.com/dilukshann7/dlman" },
  ],
} as const;
