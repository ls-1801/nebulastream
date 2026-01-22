# NebulaStream Rig - PR Workflow

**Critical rules for agents:**
- **NEVER** merge directly to main
- **ALWAYS** push changes to a feature branch
- **ONLY** create a PR when explicitly requested by the user
- **ALWAYS** call `gt done` when work is complete

## Dispatching Work

Instead of the standard `gt sling <issue> nebulastream`, use:

```bash
# Option 1: Explicit formula
gt sling mol-polecat-work-pr --on <issue-id> nebulastream

# Option 2: Helper script
./nebulastream/.gt-sling-pr <issue-id>
```

This ensures polecats create GitHub PRs instead of submitting to the refinery.

## Workflow Differences

| Standard Workflow | PR Workflow (this rig) |
|-------------------|------------------------|
| `gt done` → MQ → Refinery merges | `gt done` → push to branch → PR on request |
| Refinery auto-merges to main | Never merge to main directly |
| No code review required | Code review on PR (when created) |

## Configuration

- `polecat_formula`: mol-polecat-work-pr
- `auto_restart`: false (refinery disabled)
- Refinery: stopped

## Key Files

- `.gt-sling-pr` - Helper script for PR workflow dispatch
- `config.json` - Rig configuration
