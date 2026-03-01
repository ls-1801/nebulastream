# Sourcegraph Integration Guide for NebulaStream

This document describes how to enable Sourcegraph code intelligence for the NebulaStream project.

## Overview

Sourcegraph is a code search and navigation platform that provides:
- Cross-repository code search
- Precise code navigation ("Go to definition", "Find references")
- Cody AI assistant for code understanding and generation

## Deployment Options

### Recommended: Sourcegraph Cloud (Managed)

**Best for**: Teams wanting minimal maintenance overhead.

Sourcegraph Cloud is a fully managed solution where Sourcegraph handles maintenance, monitoring, and upgrades. All infrastructure is hosted on Google Cloud Platform with:
- Fully segregated GCP environments per customer
- Encryption at rest for all storage volumes
- Encrypted data transport from code host to cloud

**Requirements**: Code hosts must be accessible from Sourcegraph's managed environment.

### Alternative: Self-Hosted Options

| Option | Best For | Notes |
|--------|----------|-------|
| **Kubernetes Helm** | Multi-node, scalable deployments | Most robust, requires K8s expertise |
| **Docker Compose** | Single-node simplicity | Good when K8s complexity isn't needed |
| **Machine Images** | Quick AWS/GCE deployment | Pre-configured but not customizable |

**Self-hosted benefits**: Code never leaves your environment; Sourcegraph employees have no access to customer code.

**Important**: ARM/ARM64 architectures are not supported for production deployments.

## Pricing Tiers

| Tier | Price | Features |
|------|-------|----------|
| **Free** | $0 | Basic code search, limited features |
| **Enterprise Starter** | $19/user/month | Up to 50 devs, 100 repos, 5GB storage |
| **Enterprise** | Custom | Full features, large teams, SSO, etc. |

Note: Cody AI is now enterprise-only. Free tier provides unlimited autocompletion and 200 chats/prompts per month.

## Repository Integration

### Connecting GitHub Repositories

1. Navigate to **Site Admin > Code hosts > GitHub**
2. Configure authentication (GitHub App or Personal Access Token)
3. Select repositories to sync

Both GitHub.com and GitHub Enterprise receive Tier 1 support:
- Repository syncing up to 100,000 repositories
- Permissions syncing for up to 10,000 users
- "Login with GitHub" authentication
- Private repository support

## C++ Code Intelligence Setup

NebulaStream uses CMake, making SCIP-clang the recommended indexer for precise code navigation.

### Prerequisites

- **scip-clang**: Download from [sourcegraph/scip-clang releases](https://github.com/sourcegraph/scip-clang/releases)
- **src-cli** v4.5+: Sourcegraph CLI for uploading indexes
- **Disk space**: ~2MB per translation unit
- **RAM**: ~2GB per core

### Step 1: Generate Compilation Database

Since NebulaStream uses CMake, generate the compilation database:

```bash
cmake -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON
# Or for existing builds:
cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON
```

This creates `build/compile_commands.json`.

### Step 2: Build the Project

scip-clang requires access to generated code, so build first:

```bash
cmake --build build
```

### Step 3: Run the Indexer

**Important**: Run from the project root, not from a subdirectory.

```bash
scip-clang --compdb-path=build/compile_commands.json
```

This generates `index.scip` in the project root.

### Step 4: Upload the Index

```bash
# Set your Sourcegraph instance URL and access token
export SRC_ENDPOINT=https://your-sourcegraph-instance.com
export SRC_ACCESS_TOKEN=your-access-token

# Upload the index
src code-intel upload -file=index.scip
```

### Automating Index Updates

For CI/CD integration, add to your pipeline:

```yaml
# Example GitHub Actions step
- name: Generate SCIP index
  run: |
    cmake -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON
    cmake --build build
    scip-clang --compdb-path=build/compile_commands.json
    src code-intel upload -file=index.scip
```

## IDE Plugin Installation

### VS Code

1. Open VS Code
2. Go to Extensions (Ctrl+Shift+X)
3. Search for "Sourcegraph"
4. Install the official Sourcegraph extension
5. Configure with your Sourcegraph instance URL

### JetBrains IDEs (CLion, etc.)

Cody supports all JetBrains IDEs including CLion:

1. Open **Settings/Preferences > Plugins**
2. Search for "Cody" or "Sourcegraph"
3. Install the plugin
4. Configure your Sourcegraph instance in **Settings > Tools > Sourcegraph**

**Supported IDEs**: IntelliJ IDEA, CLion, PyCharm, WebStorm, GoLand, RubyMine, PhpStorm, Rider, DataGrip, RustRover, Aqua, DataSpell

### Browser Extension

Available for Chrome, Firefox, and Safari:
1. Install from your browser's extension store
2. Configure with your Sourcegraph instance URL
3. Enables code navigation on GitHub, GitLab, etc.

## Quick Start Recommendation

For NebulaStream, the recommended setup is:

1. **Start with Sourcegraph Cloud** for minimal operational overhead
2. **Install scip-clang** and set up CI pipeline for index generation
3. **Install CLion plugin** for IDE integration (since NebulaStream is C++)
4. **Enable Cody** for AI-assisted code understanding

## Open Questions to Consider

Before implementation, decide on:

1. **Cloud vs Self-Hosted**: Does code need to stay on-premises?
2. **Team or Personal**: Is this for individual use or team-wide?
3. **Budget**: Free tier or Enterprise features needed?
4. **Target IDE**: CLion? VS Code? Both?

## References

- [Sourcegraph Deployment Docs](https://sourcegraph.com/docs/admin/deploy)
- [SCIP-clang Repository](https://github.com/sourcegraph/scip-clang)
- [Cody JetBrains Plugin](https://plugins.jetbrains.com/plugin/9682-cody-ai-by-sourcegraph)
- [Sourcegraph Pricing](https://sourcegraph.com/pricing)
- [Code Intelligence Docs](https://docs.sourcegraph.com/code_intelligence)
