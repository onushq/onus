//! Two risk facts per row for the lane classifier: how many other
//! components depend on what the row changed, and whether a test or a trace
//! reaches the code it changes (ADR 0014).
//!
//! Test reach is per file: a file counts as tested when a test exercises
//! anything in it. It is cheap and stable, and a test that imports a module
//! covers additions to it. `untested` means no test touches the file at
//! all; it is not line coverage.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use onus_core::ids;
use onus_core::{ChangeKind, CodebaseMap, Confidence, SemanticChange, Side, TestReach};

use crate::ctx::Ctx;

pub fn fill(ctx: &Ctx, rows: &mut [SemanticChange]) {
    let head_users = users(ctx, ctx.head);
    let base_users = users(ctx, ctx.base);
    let files = Files::new(ctx);
    for r in rows.iter_mut() {
        r.hints.dependent_components = dependent_components(ctx, r, &head_users, &base_users);
        r.hints.tested = files.reach(ctx, r);
    }
}

/// Target id → the components of the files that use it.
type Users<'m> = BTreeMap<&'m str, BTreeSet<String>>;

fn users<'m>(ctx: &Ctx, map: &'m CodebaseMap) -> Users<'m> {
    let mut out: Users = BTreeMap::new();
    let mut component: HashMap<&str, String> = HashMap::new();
    for e in &map.edges {
        let comps = out.entry(e.to.as_str()).or_default();
        for s in &e.sites {
            let c = component
                .entry(s.file.as_str())
                .or_insert_with(|| ctx.component_of_path(&s.file));
            comps.insert(c.clone());
        }
    }
    out
}

fn dependent_components(ctx: &Ctx, r: &SemanticChange, head: &Users, base: &Users) -> u32 {
    let subject = r.subject.as_str();
    // A row about a whole component does not count the component's users.
    if ctx.components.contains_key(subject) && !subject.contains(':') {
        return 0;
    }
    let removed = !ctx.head_syms.contains_key(subject) && ctx.base_syms.contains_key(subject);
    let users = if removed { base } else { head };
    let mut comps: BTreeSet<&str> = BTreeSet::new();
    if let Some(c) = users.get(subject) {
        comps.extend(c.iter().map(String::as_str));
    }
    // Members of a class or an object count for it.
    let prefix = format!("{subject}.");
    for (_, c) in users
        .range(prefix.as_str()..)
        .take_while(|(to, _)| to.starts_with(&prefix))
    {
        comps.extend(c.iter().map(String::as_str));
    }
    comps.remove(r.component.as_deref().unwrap_or("root"));
    comps.remove("root");
    comps.len() as u32
}

/// The head files tests exercise and the head files traces have observed.
struct Files {
    tested: BTreeSet<String>,
    traced: BTreeSet<String>,
}

impl Files {
    fn new(ctx: &Ctx) -> Self {
        // Module id → its file.
        let mut modules: HashMap<String, &str> = HashMap::new();
        for f in &ctx.head.files {
            let comp = f.component_id.as_deref().unwrap_or("root");
            let dir = ctx
                .components
                .get(comp)
                .map(|c| onus_lang_ts::component_dir(&c.roots))
                .unwrap_or_default();
            let rel = if dir.is_empty() {
                f.path.as_str()
            } else {
                f.path
                    .strip_prefix(&format!("{dir}/"))
                    .unwrap_or(f.path.as_str())
            };
            modules.insert(ids::module_id(comp, rel), f.path.as_str());
        }
        let file_of = |id: &str| -> Option<String> {
            ctx.head_syms
                .get(id)
                .and_then(|s| s.loc.as_ref())
                .map(|l| l.file.clone())
                .or_else(|| modules.get(ids::module_of(id)).map(|f| f.to_string()))
        };
        let tested = ctx
            .head
            .tests
            .iter()
            .flat_map(|t| &t.exercises)
            .filter_map(|id| file_of(id))
            .collect();
        let mut traced = BTreeSet::new();
        for e in ctx
            .head
            .edges
            .iter()
            .filter(|e| e.confidence == Confidence::Traced)
        {
            traced.extend(e.sites.iter().map(|s| s.file.clone()));
            traced.extend(file_of(&e.to));
        }
        Files { tested, traced }
    }

    fn reach(&self, ctx: &Ctx, r: &SemanticChange) -> Option<TestReach> {
        if !matches!(
            r.kind,
            ChangeKind::Internal
                | ChangeKind::Additive
                | ChangeKind::Breaking
                | ChangeKind::SecuritySensitive
        ) {
            return None;
        }
        let code: BTreeSet<&str> = r
            .locations
            .iter()
            .filter(|l| l.side == Side::Head)
            .map(|l| l.file.as_str())
            .filter(|f| ctx.head_files.get(f).is_some_and(|f| !f.is_test) && !ctx.is_test_data(f))
            .collect();
        if code.is_empty() {
            None
        } else if code.iter().all(|f| self.tested.contains(*f)) {
            Some(TestReach::Tested)
        } else if code
            .iter()
            .all(|f| self.tested.contains(*f) || self.traced.contains(*f))
        {
            Some(TestReach::Traced)
        } else {
            Some(TestReach::Untested)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::change;
    use crate::testing::{component, ctx, edge, file, function, map, test};
    use crate::text::TreeDiff;
    use onus_core::{ChangeLevel, Location};
    use serde_json::json;

    fn row(kind: ChangeKind, subject: &str, component: &str, file: &str) -> SemanticChange {
        change(
            kind,
            "x",
            ChangeLevel::Behavior,
            subject,
            Some(component),
            "",
            subject.into(),
            String::new(),
            vec![Location::head(file, 1, 2)],
        )
    }

    #[test]
    fn tests_and_traces_reach_files() {
        let head = map(json!({
            "components": [component("logger"), component("billing")],
            "files": [
                file("logger", "src/logger.ts"), file("logger", "src/format.ts"),
                file("logger", "src/redact.ts"), file("logger", "src/logger.test.ts"),
                file("billing", "src/bill.ts"),
            ],
            "symbols": [
                function("logger:src/logger.ts#createLogger", 1, "a"),
                function("logger:src/format.ts#format", 1, "b"),
                function("logger:src/redact.ts#redact", 1, "c"),
            ],
            "tests": [test("logger", "src/logger.test.ts", &["logger:src/logger.ts#createLogger"])],
            "edges": [edge(
                "billing:src/bill.ts#bill", "logger:src/format.ts#format",
                "packages/billing/src/bill.ts", "traced",
            )],
        }));
        let base = map(json!({}));
        let text = TreeDiff::default();
        let c = ctx(&base, &head, &text, None);
        let mut rows = vec![
            row(
                ChangeKind::Internal,
                "logger",
                "logger",
                "packages/logger/src/logger.ts",
            ),
            row(
                ChangeKind::Internal,
                "logger",
                "logger",
                "packages/logger/src/format.ts",
            ),
            row(
                ChangeKind::Additive,
                "logger",
                "logger",
                "packages/logger/src/redact.ts",
            ),
            row(
                ChangeKind::Config,
                "logger",
                "logger",
                "packages/logger/src/redact.ts",
            ),
            row(
                ChangeKind::Internal,
                "logger",
                "logger",
                "packages/logger/src/logger.test.ts",
            ),
        ];
        fill(&c, &mut rows);
        let reach: Vec<Option<TestReach>> = rows.iter().map(|r| r.hints.tested).collect();
        assert_eq!(
            reach,
            [
                Some(TestReach::Tested),
                Some(TestReach::Traced),
                Some(TestReach::Untested),
                None,
                None,
            ]
        );
    }

    #[test]
    fn dependent_components_leave_out_the_rows_own() {
        let used = "logger:src/logger.ts#createLogger";
        let head = map(json!({
            "components": [component("logger"), component("billing"), component("orders")],
            "symbols": [function(used, 1, "a")],
            "edges": [
                edge("billing:src/a.ts", used, "packages/billing/src/a.ts", "static"),
                edge("orders:src/b.ts", used, "packages/orders/src/b.ts", "static"),
                edge("logger:src/c.ts", used, "packages/logger/src/c.ts", "static"),
                edge("orders:src/b.ts", &format!("{used}.child"), "packages/orders/src/b.ts", "static"),
            ],
        }));
        let base = map(json!({}));
        let text = TreeDiff::default();
        let c = ctx(&base, &head, &text, None);
        let mut rows = vec![
            row(
                ChangeKind::Internal,
                used,
                "logger",
                "packages/logger/src/logger.ts",
            ),
            row(
                ChangeKind::Internal,
                "logger",
                "logger",
                "packages/logger/src/logger.ts",
            ),
        ];
        fill(&c, &mut rows);
        assert_eq!(rows[0].hints.dependent_components, 2);
        assert_eq!(rows[1].hints.dependent_components, 0);
    }
}
