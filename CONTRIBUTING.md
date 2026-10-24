# Contributing to DLMan

Thank you for your interest in contributing to DLMan! We welcome contributions of all kinds.

## 🚀 Quick Start

```bash
# Clone the repository
git clone https://github.com/dilukshann7/dlman.git
cd dlman

# Install dependencies
pnpm install

# Run the desktop app in development mode
pnpm tauri dev
```

## 📋 Prerequisites

- [Node.js](https://nodejs.org/) v20+
- [Rust](https://rustup.rs/) 1.75+
- [pnpm](https://pnpm.io/) v9+

## 🏗️ Project Structure

```
dlman/
├── apps/
│   ├── desktop/          # Tauri + React desktop app
│   │   ├── src/          # React frontend
│   │   └── src-tauri/    # Rust backend
│   ├── cli/              # CLI application
│   └── extension/        # Browser extension
├── crates/
│   ├── dlman-core/       # Core download engine
│   └── dlman-types/      # Shared types
└── docs/                 # Documentation
```

## 💻 Development

### Desktop App

```bash
# Development mode with hot reload
pnpm tauri dev

# Build for production
pnpm tauri build
```

### CLI

```bash
# Run CLI in development
cargo run -p dlman-cli -- --help

# Build release binary
cargo build -p dlman-cli --release
```

### Browser Extension

```bash
# Chrome development
pnpm --filter @dlman/extension dev

# Firefox development
pnpm --filter @dlman/extension dev:firefox

# Build for production
pnpm --filter @dlman/extension build:chrome
pnpm --filter @dlman/extension build:firefox
```

## 📝 Code Style

### TypeScript/React
- Strict mode enabled, avoid `any` types
- One component per file
- Use functional components with hooks
- Prefer named exports

### Rust
- Follow standard clippy lints
- Keep functions focused and small
- Document public APIs

### General
- Keep files under 300 lines when possible
- Write descriptive commit messages
- Add comments for complex logic

## ✅ Testing

```bash
# Run Rust tests
cargo test

# Type check frontend
cd apps/desktop && pnpm tsc --noEmit
```

## 📤 Submitting Changes

### For Bug Fixes

1. Check if an issue already exists
2. Create an issue if not
3. Fork the repository
4. Create a branch: `git checkout -b fix/issue-description`
5. Make your changes
6. Test thoroughly
7. Submit a pull request

### For New Features

1. Open an issue to discuss the feature first
2. Wait for feedback from maintainers
3. Fork and create a branch: `git checkout -b feature/feature-name`
4. Implement the feature
5. Add tests if applicable
6. Submit a pull request

### Pull Request Guidelines

- Keep PRs focused on a single change
- Update documentation if needed
- Ensure all tests pass
- Follow the existing code style
- Write a clear PR description

## 📚 Documentation

- [Architecture](docs/ARCHITECTURE.md) — System design overview
- [Core Engine](docs/CORE.md) — Download engine internals
- [CLI](docs/CLI.md) — Command-line interface docs
- [Extension](docs/EXTENSION.md) — Browser extension docs
- [Development](docs/DEVELOPMENT.md) — Detailed dev setup

## 🐛 Reporting Bugs

When reporting bugs, please include:

1. DLMan version
2. Operating system and version
3. Steps to reproduce
4. Expected vs actual behavior
5. Screenshots or logs if applicable

## 💡 Feature Requests

We're open to feature ideas! When suggesting features:

1. Check existing issues first
2. Describe the use case
3. Explain why it would be valuable
4. Consider implementation complexity

## 🤝 Code of Conduct

- Be respectful and inclusive
- Constructive feedback only
- Help others learn and grow
- Focus on the work, not the person

## ❓ Questions?

- Open a [GitHub Discussion](https://github.com/dilukshann7/dlman/discussions)
- Check existing [issues](https://github.com/dilukshann7/dlman/issues)

---

Thank you for contributing! 🎉
