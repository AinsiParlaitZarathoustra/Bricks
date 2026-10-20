# Permissions

This page is a summary. The reference is [Tool API, Permission Levels](/docs/api-tools#permission-levels) on the documentation site.

- A permission policy implements `PermissionPolicy` and decides each tool call from its `PermissionRequest`. Set it with `Agent::builder().permission_policy(...)`.
- If no policy is set, the builder uses `AllowAll`: every call is allowed. Set a policy that fits the task.
- The built-in policies are `AllowAll`, `AllowReadOnly`, `DenyAll`, `RuleBased` and `InteractivePolicy`.
- Each tool has a permission level (`None`, `ReadOnly`, `Write`, `Execute`, `Dangerous`, `Forbidden`) that the policy sees with the call.
