# CLEANUP-7: Fix jsexec lambda metrics tests (6 failures)

blocked_by: []
unlocks: [CLEANUP-8]
difficulty: trivial
cascades_to: []

## Problem

All 6 failing tests hit the same error:
```
Failed to insert test function: Database(SqliteError {
  code: 1,
  message: "table functions has no column named environment"
})
```

### Failing tests
1. `test_function_metrics`
2. `test_metrics_with_time_range`
3. `test_cleanup_old_records`
4. `test_global_stats`
5. `test_no_invocations`
6. `test_record_and_retrieve_invocation`

### Root cause
Column name mismatch between test helper and schema:
- **Migration** (`001_create_functions_table.sql` line 11): column is `environment_variables`
- **Test helper** (`src/lambda/metrics.rs` line 508): INSERT uses `environment`

## Fix

**One line** in `src/lambda/metrics.rs:508`:
```sql
-- Before:
INSERT INTO functions (..., environment, ...)
-- After:
INSERT INTO functions (..., environment_variables, ...)
```
