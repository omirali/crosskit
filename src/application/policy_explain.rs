// Use case: explain which scope decision the configured policy makes for one path. It
// works on the path string alone and never touches the filesystem.
use crate::domain::{IndexPolicy, NormalizedPath, PathError, PolicyClass, RuleOrigin};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyExplanation {
    path: NormalizedPath,
    class: PolicyClass,
    matched_rule: Option<MatchedRule>,
}

impl PolicyExplanation {
    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    pub fn class(&self) -> PolicyClass {
        self.class
    }

    /// `None` when no rule covered the path and the safe default excluded it.
    pub fn matched_rule(&self) -> Option<&MatchedRule> {
        self.matched_rule.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedRule {
    class: PolicyClass,
    pattern: String,
    origin: RuleOrigin,
}

impl MatchedRule {
    pub fn class(&self) -> PolicyClass {
        self.class
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn origin(&self) -> RuleOrigin {
        self.origin
    }
}

pub struct PolicyExplainService<'a> {
    policy: &'a IndexPolicy,
}

impl<'a> PolicyExplainService<'a> {
    pub fn new(policy: &'a IndexPolicy) -> Self {
        Self { policy }
    }

    pub fn execute(&self, absolute_path: &str) -> Result<PolicyExplanation, PathError> {
        let path = NormalizedPath::from_absolute(absolute_path)?;
        let decision = self.policy.decide(&path);
        Ok(PolicyExplanation {
            class: decision.class(),
            matched_rule: decision.rule().map(|rule| MatchedRule {
                class: rule.class(),
                pattern: rule.pattern().as_str().to_string(),
                origin: rule.origin(),
            }),
            path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::PolicyExplainService;
    use crate::domain::{IndexPolicy, PathPattern, PolicyClass, PolicyRule, RuleOrigin};

    fn policy() -> IndexPolicy {
        IndexPolicy::new(
            vec![
                PolicyRule::new(
                    PolicyClass::Exclude,
                    PathPattern::parse("**/*.key").unwrap(),
                    RuleOrigin::BuiltIn,
                ),
                PolicyRule::new(
                    PolicyClass::Include,
                    PathPattern::parse("/home/alice/Documents").unwrap(),
                    RuleOrigin::Configuration,
                ),
            ],
            1_024,
        )
    }

    #[test]
    fn explains_paths_that_do_not_exist_without_touching_the_filesystem() {
        let explanation = PolicyExplainService::new(&policy())
            .execute("/nonexistent/vault/../vault/server.key")
            .unwrap();

        assert_eq!(explanation.path().as_str(), "/nonexistent/vault/server.key");
        assert_eq!(explanation.class(), PolicyClass::Exclude);
        let rule = explanation.matched_rule().unwrap();
        assert_eq!(rule.pattern(), "**/*.key");
        assert_eq!(rule.origin(), RuleOrigin::BuiltIn);
    }

    #[test]
    fn reports_the_safe_default_when_nothing_matches() {
        let explanation = PolicyExplainService::new(&policy())
            .execute("/home/bob/notes.md")
            .unwrap();

        assert_eq!(explanation.class(), PolicyClass::Exclude);
        assert!(explanation.matched_rule().is_none());
    }
}
