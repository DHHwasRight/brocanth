# `admin-actions`

"Delete selected" on every admin list, with a confirmation page.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`admin`](admin.md), off until you tick it |
| Needs | nothing |
| After | nothing |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none: actions are in `brocanth-admin` |
| Without it | no "Delete selected"; a typed action can still be added by hand |

## What it writes

Files: none.

Slots:

- src/plugins/mod.rs (admin_panels): `config.action(BulkAction::delete_selected())`
- AGENTS.md: `## Admin actions`

## What the admin does with an action

Actions are Django's `ModelAdmin.actions`, in `brocanth_admin::actions`.
Every admin has them; this module only turns on "Delete selected".

- A list with at least one action this person may run draws a checkbox per
  row and a menu. The checkboxes join the action form through the HTML
  `form` attribute, so the page works with no script.
- The first POST draws a confirmation page: the records the rules admit,
  and how many selected records are left out. It writes nothing.
- The confirmed POST runs the action and redirects (303) to the list with a
  line: `Delete selected: 2 posts.`
- Every action names the row rule it needs, `RowAction::Update` or
  `RowAction::Delete`. The screen policy is asked for the model and for
  each record, and the row policy's condition guards every read and write,
  as on the single-record screens. A record the rules refuse is skipped.
- Every write goes to the journal (`AdminConfig::journal`), so `audit`
  records each record an action changed.

Three kinds:

| Kind | What it does |
|---|---|
| `BulkAction::delete_selected()` | deletes each selected record; shown only on a model that allows a delete |
| `BulkAction::set(key, label, [(column, value)])` | sets fixed values; `Admin::new` panics on a column the model lacks |
| `BulkAction::custom(key, label, needs, f)` | your async function over the selected records, which writes through `cx.update` and `cx.delete` |

Typed per model, in `src/plugins/admin/mod.rs`:

```rust
config.model("post", |m| {
    m.action(BulkAction::set("publish", "Publish selected", [("published", true)]))
        .without_action("delete_selected")
})
```

## Decisions

| Question | Decision | Reason |
|---|---|---|
| A module, or only the library? | both: the library for every admin, this module for "Delete selected" | An action over every model is a choice a project makes once. A typed action is code, so it is the project's, in its admin file. |
| A toggle in `src/modules/admin_actions.rs`? | no | The module writes one line. `brocanth remove admin-actions` takes it out. |
| A confirmation page for every action? | yes | Django does it for delete only. A bulk write is hard to undo, and the page shows which records the rules leave out. |
| A typo in a `set` column? | a panic in `Admin::new` | At start, not on the first click. |
| Most records per action | 1 000 | A list page shows a few hundred; a POST with more keys was not made from a list. |

## Not built

- Inlines (Django's `TabularInline`): the related rows of a record on its
  edit form. Next on the admin's list.
- An action that asks for input on its confirmation page (a reason, a
  date).

## Settings

None.

## Read more

- [The backoffice](../backoffice.md)
- Source: [`crates/brocanth-admin/src/actions.rs`](../../crates/brocanth-admin/src/actions.rs), [`crates/brocanth-admin/src/scaffold.rs`](../../crates/brocanth-admin/src/scaffold.rs)
