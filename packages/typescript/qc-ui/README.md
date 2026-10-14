# qc-ui

Reusable React web components for QC screen views and hardware controls. It receives state and semantic actions through props/hooks and contains no device transport or Tauri calls.

Native mobile clients are not required to reuse these DOM components.

Production views live in their own modules, including `coros-gig-view`, `coros-cpu-monitor`, `coros-reference-settings-pane` and `coros-official-grid`. `coros-screen-fixtures` composes these views with reference data; production code must not import fixture implementations. The development audit host is available through `@qc-remote/ui/dev`.

