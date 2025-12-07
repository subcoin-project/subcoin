# Subcoin Explorer

A web-based block explorer for the Subcoin Bitcoin node.

## Prerequisites

- [Bun](https://bun.sh/) (package manager and runtime)
- A running Subcoin node on `localhost:9944`

## Setup

```bash
cd js
bun install
```

## Development

Start the development server:

```bash
# From js/ directory
bun run --cwd packages/explorer vite --port 3002

# Or from packages/explorer/ directory
cd packages/explorer
bunx vite --port 3002
```

Open http://localhost:3002 in your browser.

## Project Structure

```
js/
├── packages/
│   ├── explorer/          # React block explorer app
│   │   ├── src/
│   │   │   ├── pages/     # Dashboard, BlockDetail, TransactionDetail, Network
│   │   │   ├── components/
│   │   │   └── App.tsx
│   │   └── vite.config.ts
│   │
│   └── shared/            # Shared RPC client and types
│       └── src/
│           ├── api/       # Subcoin RPC client wrappers
│           └── types/     # TypeScript type definitions
│
└── package.json           # Workspace root
```

## Features

- **Dashboard**: Latest blocks, network status, sync progress
- **Block Detail**: View block header, transactions with expandable inputs/outputs
- **Transaction Detail**: Full transaction breakdown
- **Network**: Connected peers, bandwidth stats, sync state

## Configuration

The explorer connects to a Subcoin node via a Vite proxy to avoid CORS issues.
The proxy is configured in `packages/explorer/vite.config.ts`:

```typescript
proxy: {
  "/rpc": {
    target: "http://localhost:9944",  // Change this for different node
    changeOrigin: true,
    rewrite: (path) => path.replace(/^\/rpc/, ""),
  },
}
```

## Tech Stack

- React 18 + TypeScript
- Vite (bundler)
- Tailwind CSS (styling)
- Bun (package manager)
