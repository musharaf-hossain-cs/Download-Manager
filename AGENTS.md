# Repository Instructions

## Slice completion commits

When completing one or more implementation slices, use this exact commit subject format:

```text
type(scope): complete SNNN short description
```

For a contiguous range of slices, use:

```text
type(scope): complete SNNN-SNNN short description
```

The slice ID or range must appear immediately after `complete`. Keep the Conventional Commit type and an applicable scope. Do not move the slice ID to the end of the subject or insert `slice`/`slices` before it.

Examples:

```text
feat(engine): complete S135 HTML crawler
feat(engine): complete S128-S131 scheduler policy slices
```
