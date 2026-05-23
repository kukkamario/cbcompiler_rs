# Close / Complete a Feature Design

Close an FD by marking it complete (or closed/deferred), archiving the file, and updating the index.

## Argument

The argument should contain:
- **FD number** (required-ish): e.g. `1` or `FD-001`
- **Disposition** (optional): `complete` (default), `closed`, or `deferred`
- **Notes** (optional): any additional context

Examples:
- `/fd-close 1` — mark FD-001 as Complete
- `/fd-close 2 deferred blocked on X` — mark FD-002 as Deferred
- `/fd-close 3 closed superseded by FD-005` — mark FD-003 as Closed
- `/fd-close` (no args) — infer from conversation context

Parse the argument: ``

## Inferring the FD

If no FD number provided, infer from conversation context:
- Look at which FD was most recently discussed or worked on
- If exactly one FD is obvious, use it and state which one
- If ambiguous, ask the user

## Steps

### 1. Find and read the FD file

- Glob for `docs/features/FD-{number}_*.md`
- Read to get title, current status
- If already archived or not found, report and stop

### 2. Update the FD file

- Set `**Status:**` to `Complete`, `Closed`, or `Deferred`
- For Complete: add `**Completed:** {today YYYY-MM-DD}` after Status
- For Closed/Deferred: add `**Closed:** {today}` if not present

### 3. Update FEATURE_INDEX.md

- Read `docs/features/FEATURE_INDEX.md`
- Remove FD's row from **Active Features** table
- Add to appropriate section:
  - **Complete** → add to top of `## Completed` table with date and notes
  - **Closed/Deferred** → add to top of `## Deferred / Closed` table with status and notes

### 4. Archive the file

- Move FD file to `docs/features/archive/`

### 5. Commit

Commit all changes related to this FD in a single atomic commit:
- Check `git status` for uncommitted changes related to the FD implementation (code files modified during this session)
- Stage implementation files, the archived FD, deleted original FD path, and `FEATURE_INDEX.md`
- Commit with message: `FD-{number}: {title}`

### 6. Summary

Report:
- FD number and title
- Disposition (Complete / Closed / Deferred)
- Status updated in FD file
- Moved from Active to the appropriate section in index
- Archived to `docs/features/archive/`
- Committed: {short hash}
