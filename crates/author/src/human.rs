//! A person's own verdict on an app, recorded so it means something later.
//!
//! Some things a machine cannot settle. Whether the spacing looks right,
//! whether the wording is friendly, whether the app is the one the person
//! pictured -- those need a human to look. Until now there was nowhere to put
//! that answer, so it lived in someone's memory and was gone by the next
//! release.
//!
//! What this is careful about is the other half. A person saying "looks good"
//! must never quietly stand in for a check that failed. The machine verdicts
//! and the human verdict are separate records, and the rule between them runs
//! one way only: a human may waive a stated *preference*, and may not waive a
//! functional, security or portability failure. So "I know the button is on
//! the left, ship it" is a waiver; "the sandbox escape is fine" is not, and
//! recording it is refused rather than accepted quietly.
//!
//! Every verdict names the exact artifact it is about, by digest. An approval
//! of one build says nothing about the next one -- which is the point, because
//! the failure this prevents is an old approval being read as covering new
//! bytes.

use serde::{Deserialize, Serialize};

/// What a person decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    /// The app does what was wanted, as far as a person can tell by using it.
    Accepted,
    /// It does not. The notes say what is wrong.
    Rejected,
    /// Something is off, and the person is choosing to ship anyway. Only ever
    /// valid over a preference -- see [`Waiver`].
    AcceptedWithWaiver,
}

/// Why a machine check is being overridden, and of what kind.
///
/// The kind is the whole point. A preference is a matter of taste and its
/// owner may overrule it; the other three are not, and this type is what makes
/// that difference checkable rather than a matter of everyone remembering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiver {
    /// Which requirement or check is being waived, by its id.
    pub id: String,
    /// The kind of thing being waived. Only `Preference` may be waived.
    pub kind: Concern,
    /// The person's reason, in their own words. Required: a waiver without a
    /// reason is indistinguishable from not having looked.
    pub reason: String,
}

/// What class of thing a check belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Concern {
    /// Taste: layout, wording, colour, ordering. A person may overrule this.
    Preference,
    /// The app does not do what was asked. Not waivable.
    Functional,
    /// A permission, sandbox or capability failure. Not waivable.
    Security,
    /// It does not work on a platform it must work on. Not waivable.
    Portability,
}

impl Concern {
    /// May a person's say-so override this?
    ///
    /// Only taste. The other three are the reasons the checks exist, and a
    /// product that lets a reviewer click past them has checks in name only.
    pub fn is_waivable(self) -> bool {
        matches!(self, Concern::Preference)
    }

    /// How to name this in a refusal, in the person's language.
    pub fn plain_name(self) -> &'static str {
        match self {
            Concern::Preference => "a preference",
            Concern::Functional => "something the app does not do",
            Concern::Security => "a permission or sandbox failure",
            Concern::Portability => "a platform it does not work on",
        }
    }
}

/// One person's recorded verdict on one exact artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acceptance {
    /// Schema name, so a reader knows what shape this is.
    pub schema: String,
    /// The exact bytes this verdict is about. An approval of one build says
    /// nothing about another, and this is what makes that enforceable.
    pub artifact_digest: String,
    /// The request the app was built from, so the verdict can be read without
    /// the surrounding conversation.
    pub request: String,
    /// The requirement ids that were in force when this was decided. A later
    /// requirement change makes a new revision rather than editing this one.
    pub requirements: Vec<String>,
    /// Who looked, in whatever way the team names people.
    pub reviewer: String,
    /// What they were doing: "developer", "designer", "the person who asked".
    pub reviewer_role: String,
    /// When, as an ISO date.
    pub at: String,
    /// What they decided.
    pub decision: Decision,
    /// Anything they want the next reader to know.
    pub notes: String,
    /// Machine checks being overridden, if any.
    pub waivers: Vec<Waiver>,
    /// Which revision of this case this is. A changed requirement bumps it.
    pub revision: u32,
}

/// Why a verdict could not be recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Someone tried to wave through a failure that is not theirs to wave.
    NotWaivable {
        /// The check they tried to waive.
        id: String,
        /// What kind of thing it is.
        kind: Concern,
    },
    /// A waiver with no reason given.
    NoReason {
        /// The check whose waiver had no reason.
        id: String,
    },
    /// An acceptance that carries no name, date, artifact or reviewer.
    Incomplete {
        /// Which field is missing.
        field: &'static str,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Written to be read by the person who just tried it, so it says
            // what they may do instead rather than only what they may not.
            Refusal::NotWaivable { id, kind } => write!(
                f,
                "{id} is {}, and that is not something a sign-off can wave \
                 through. Fix it, or change the requirement on purpose -- \
                 which makes a new revision and is recorded as one. Only a \
                 preference can be waived.",
                kind.plain_name()
            ),
            Refusal::NoReason { id } => write!(
                f,
                "waiving {id} needs a reason. A waiver with nothing written \
                 in it cannot be told apart from nobody having looked."
            ),
            Refusal::Incomplete { field } => write!(
                f,
                "an acceptance needs {field}: without it the record cannot say \
                 who decided what, about which build."
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// What a person is asked to do before deciding.
///
/// A sign-off on "does it look right?" is worth very little. A sign-off on
/// "open it, add three items, close it, open it again, and say whether they
/// are still there" is worth something, because the next person can do the
/// same thing and get the same answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanTask {
    /// Stable id, so a verdict can point at the exact task.
    pub id: String,
    /// What to do, in steps a person can follow without guessing.
    pub steps: Vec<String>,
    /// What they should see if it worked.
    pub expected: String,
}

/// Build the verdict, refusing the ones that would make the record a lie.
///
/// Every refusal here is a case where recording the verdict would leave a file
/// that reads as approval and is not one.
#[allow(clippy::too_many_arguments)]
pub fn record(
    artifact_digest: &str,
    request: &str,
    requirements: Vec<String>,
    reviewer: &str,
    reviewer_role: &str,
    at: &str,
    decision: Decision,
    notes: &str,
    waivers: Vec<Waiver>,
    revision: u32,
) -> Result<Acceptance, Refusal> {
    // A record that cannot say what it is about, or who said it, is not a
    // record. These are checked before the waivers so the message a person
    // gets is about the thing they most likely forgot.
    if artifact_digest.trim().is_empty() {
        return Err(Refusal::Incomplete {
            field: "the digest of the exact build being accepted",
        });
    }
    if reviewer.trim().is_empty() {
        return Err(Refusal::Incomplete {
            field: "the name of whoever looked",
        });
    }
    if reviewer_role.trim().is_empty() {
        return Err(Refusal::Incomplete {
            field: "what the reviewer was doing (developer, designer, the person who asked)",
        });
    }
    if at.trim().is_empty() {
        return Err(Refusal::Incomplete {
            field: "the date it was decided",
        });
    }

    for waiver in &waivers {
        // The rule of the whole module: taste may be overruled, the rest may
        // not. A reviewer who disagrees with a functional check changes the
        // requirement on purpose, which is a revision and is visible.
        if !waiver.kind.is_waivable() {
            return Err(Refusal::NotWaivable {
                id: waiver.id.clone(),
                kind: waiver.kind,
            });
        }
        if waiver.reason.trim().is_empty() {
            return Err(Refusal::NoReason {
                id: waiver.id.clone(),
            });
        }
    }

    Ok(Acceptance {
        schema: "krate.accept.v1".to_string(),
        artifact_digest: artifact_digest.to_string(),
        request: request.to_string(),
        requirements,
        reviewer: reviewer.to_string(),
        reviewer_role: reviewer_role.to_string(),
        at: at.to_string(),
        decision,
        notes: notes.to_string(),
        waivers,
        revision,
    })
}

impl Acceptance {
    /// Does this verdict cover these exact bytes?
    ///
    /// The question that makes the digest worth recording. An approval of an
    /// earlier build must not be read as covering a later one, however small
    /// the change was -- that is precisely how a revision ships unreviewed.
    pub fn covers(&self, artifact_digest: &str) -> bool {
        self.artifact_digest == artifact_digest
    }

    /// Is this a green light?
    ///
    /// A rejection is not, and neither is an acceptance whose waivers do not
    /// hold up -- though `record` refuses to build one of those, so this is
    /// the second line rather than the first.
    pub fn is_green(&self) -> bool {
        matches!(
            self.decision,
            Decision::Accepted | Decision::AcceptedWithWaiver
        ) && self.waivers.iter().all(|w| w.kind.is_waivable())
    }

    /// The same case after a requirement changed on purpose.
    ///
    /// Changing what was asked for does not edit the old verdict -- it makes a
    /// new one, at a higher revision, against the new requirements. The old
    /// record stays exactly as it was, which is what makes the history worth
    /// reading.
    pub fn revise(&self, requirements: Vec<String>, at: &str) -> Acceptance {
        Acceptance {
            schema: self.schema.clone(),
            artifact_digest: self.artifact_digest.clone(),
            request: self.request.clone(),
            requirements,
            reviewer: self.reviewer.clone(),
            reviewer_role: self.reviewer_role.clone(),
            at: at.to_string(),
            // A changed requirement is not a decision. The new revision starts
            // undecided, because nobody has looked at the app against it yet.
            decision: Decision::Rejected,
            notes: format!(
                "requirements changed from revision {}; not yet reviewed against the new ones",
                self.revision
            ),
            waivers: Vec::new(),
            revision: self.revision + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_args() -> (String, String, Vec<String>, String, String, String) {
        (
            "sha256:abc123".to_string(),
            "a checklist that saves my items".to_string(),
            vec!["req-1".to_string(), "req-2".to_string()],
            "yashraj".to_string(),
            "the person who asked".to_string(),
            "2026-09-05".to_string(),
        )
    }

    /// Test 1641: the record names the artifact, the requirements, the
    /// reviewer's role, the date and the decision. All five, or it is not a
    /// record anyone can act on later.
    #[test]
    fn a_verdict_names_the_artifact_requirements_role_date_and_decision() {
        let (digest, request, reqs, who, role, at) = ok_args();
        let a = record(
            &digest,
            &request,
            reqs.clone(),
            &who,
            &role,
            &at,
            Decision::Accepted,
            "opened it, added three items, reopened, still there",
            Vec::new(),
            1,
        )
        .expect("a complete verdict is recordable");

        assert_eq!(a.artifact_digest, digest);
        assert_eq!(a.requirements, reqs);
        assert_eq!(a.reviewer_role, role);
        assert_eq!(a.at, at);
        assert_eq!(a.decision, Decision::Accepted);
        assert_eq!(a.schema, "krate.accept.v1");
        assert!(a.is_green());
    }

    /// Each of those five missing is refused on its own, so the message names
    /// the one thing the person forgot.
    #[test]
    fn an_incomplete_verdict_is_refused_field_by_field() {
        let (digest, request, reqs, who, role, at) = ok_args();
        let cases: [(&str, &str, &str, &str); 4] = [
            ("", &request, &who, &role),
            (&digest, &request, "", &role),
            (&digest, &request, &who, ""),
            (&digest, &request, &who, &role),
        ];
        for (i, (d, r, w, ro)) in cases.iter().enumerate() {
            let when = if i == 3 { "" } else { at.as_str() };
            let got = record(
                d,
                r,
                reqs.clone(),
                w,
                ro,
                when,
                Decision::Accepted,
                "",
                Vec::new(),
                1,
            );
            assert!(
                matches!(got, Err(Refusal::Incomplete { .. })),
                "case {i} should be refused as incomplete, got {got:?}"
            );
        }
    }

    /// Test 1642, the one that matters most: a person's approval cannot stand
    /// in for a functional, security or portability failure. Only taste.
    #[test]
    fn a_person_cannot_wave_through_a_real_failure() {
        let (digest, request, reqs, who, role, at) = ok_args();
        for kind in [Concern::Functional, Concern::Security, Concern::Portability] {
            let got = record(
                &digest,
                &request,
                reqs.clone(),
                &who,
                &role,
                &at,
                Decision::AcceptedWithWaiver,
                "looks fine to me",
                vec![Waiver {
                    id: "req-2".to_string(),
                    kind,
                    reason: "shipping anyway".to_string(),
                }],
                1,
            );
            assert_eq!(
                got,
                Err(Refusal::NotWaivable {
                    id: "req-2".to_string(),
                    kind
                }),
                "{kind:?} must not be waivable"
            );
        }
    }

    /// And the case that must still work: taste is the reviewer's to overrule.
    #[test]
    fn a_person_can_wave_through_a_preference() {
        let (digest, request, reqs, who, role, at) = ok_args();
        let a = record(
            &digest,
            &request,
            reqs,
            &who,
            &role,
            &at,
            Decision::AcceptedWithWaiver,
            "",
            vec![Waiver {
                id: "req-3".to_string(),
                kind: Concern::Preference,
                reason: "the button is on the left and I want it there".to_string(),
            }],
            1,
        )
        .expect("a preference is the reviewer's to waive");
        assert!(a.is_green());
    }

    /// A waiver with nothing written in it is refused: it cannot be told apart
    /// from nobody having looked.
    #[test]
    fn a_waiver_needs_a_reason() {
        let (digest, request, reqs, who, role, at) = ok_args();
        let got = record(
            &digest,
            &request,
            reqs,
            &who,
            &role,
            &at,
            Decision::AcceptedWithWaiver,
            "",
            vec![Waiver {
                id: "req-3".to_string(),
                kind: Concern::Preference,
                reason: "   ".to_string(),
            }],
            1,
        );
        assert_eq!(
            got,
            Err(Refusal::NoReason {
                id: "req-3".to_string()
            })
        );
    }

    /// The digest is what stops an old approval covering new bytes.
    #[test]
    fn an_approval_covers_only_the_build_it_names() {
        let (digest, request, reqs, who, role, at) = ok_args();
        let a = record(
            &digest,
            &request,
            reqs,
            &who,
            &role,
            &at,
            Decision::Accepted,
            "",
            Vec::new(),
            1,
        )
        .expect("record");
        assert!(a.covers("sha256:abc123"));
        assert!(
            !a.covers("sha256:def456"),
            "an approval of one build must not cover another"
        );
    }

    /// Test 1643: changing a requirement makes a new revision rather than
    /// editing the old verdict, and the new one starts unreviewed.
    #[test]
    fn a_changed_requirement_makes_a_new_revision_that_nobody_has_approved() {
        let (digest, request, reqs, who, role, at) = ok_args();
        let first = record(
            &digest,
            &request,
            reqs,
            &who,
            &role,
            &at,
            Decision::Accepted,
            "all good",
            Vec::new(),
            1,
        )
        .expect("record");

        let second = first.revise(
            vec![
                "req-1".to_string(),
                "req-2".to_string(),
                "req-3".to_string(),
            ],
            "2026-09-06",
        );

        assert_eq!(second.revision, 2);
        assert_eq!(second.requirements.len(), 3);
        assert!(
            !second.is_green(),
            "a new requirement nobody has looked at must not inherit the old approval"
        );
        // The old record is untouched, which is what makes the history worth
        // reading at all.
        assert_eq!(first.revision, 1);
        assert!(first.is_green());
        assert_eq!(first.requirements.len(), 2);
    }

    /// The rejected-then-revised case IC-757 names: a rejection, then a fix,
    /// then a fresh approval of the new bytes -- and the rejection still
    /// standing in the record afterwards.
    #[test]
    fn a_rejection_then_a_revision_leaves_both_records() {
        let (_, request, reqs, who, role, at) = ok_args();
        let rejected = record(
            "sha256:first",
            &request,
            reqs.clone(),
            &who,
            &role,
            &at,
            Decision::Rejected,
            "the items do not survive a reopen",
            Vec::new(),
            1,
        )
        .expect("record");
        assert!(!rejected.is_green());

        // The app is fixed. New bytes, so a new verdict is needed -- the old
        // one does not reach them.
        assert!(!rejected.covers("sha256:second"));
        let accepted = record(
            "sha256:second",
            &request,
            reqs,
            &who,
            &role,
            "2026-09-06",
            Decision::Accepted,
            "items survive a reopen now",
            Vec::new(),
            1,
        )
        .expect("record");
        assert!(accepted.is_green());
        assert!(accepted.covers("sha256:second"));

        // And the rejection is still exactly what it was.
        assert_eq!(rejected.decision, Decision::Rejected);
        assert_eq!(rejected.notes, "the items do not survive a reopen");
    }

    /// A human task has to be something two people would do the same way, or
    /// the sign-off it produces is not comparable to the next one.
    #[test]
    fn a_human_task_says_what_to_do_and_what_to_expect() {
        let task = HumanTask {
            id: "task-1".to_string(),
            steps: vec![
                "open the app".to_string(),
                "add three items".to_string(),
                "close it and open it again".to_string(),
            ],
            expected: "the three items are still listed".to_string(),
        };
        assert!(!task.steps.is_empty());
        assert!(!task.expected.is_empty());
    }
}
