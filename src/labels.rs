//! The label set every project repo carries.
//! `repo init` creates these, `plan validate` checks plan labels against them.

pub struct LabelDef {
    pub name: &'static str,
    pub color: &'static str,
    pub description: &'static str,
    /// True when a plan file may put this label on an issue.
    pub plan: bool,
}

pub const LABELS: &[LabelDef] = &[
    LabelDef { name: "today", color: "0e8a16", description: "Picked for the task manager to run", plan: false },
    LabelDef { name: "in-progress", color: "fbca04", description: "The task manager is working on this issue", plan: false },
    LabelDef { name: "blocked", color: "d73a4a", description: "Waiting for a human decision", plan: false },
    LabelDef { name: "needs-sim", color: "5319e7", description: "Needs the iOS simulator lock", plan: true },
    LabelDef { name: "needs-research", color: "1d76db", description: "Research runs before development", plan: true },
    LabelDef { name: "size:S", color: "c2e0c6", description: "Small change", plan: true },
    LabelDef { name: "size:M", color: "bfdadc", description: "Medium change", plan: true },
    LabelDef { name: "priority:high", color: "b60205", description: "Runs before normal priority", plan: true },
    LabelDef { name: "priority:normal", color: "d4c5f9", description: "Default priority", plan: true },
    LabelDef { name: "priority:low", color: "ededed", description: "Runs after normal priority", plan: true },
];

pub fn is_plan_label(name: &str) -> bool {
    LABELS.iter().any(|l| l.plan && l.name == name)
}

pub fn is_state_label(name: &str) -> bool {
    LABELS.iter().any(|l| !l.plan && l.name == name)
}

pub fn plan_label_names() -> Vec<&'static str> {
    LABELS.iter().filter(|l| l.plan).map(|l| l.name).collect()
}
