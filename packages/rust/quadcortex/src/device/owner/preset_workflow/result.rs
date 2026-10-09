//! One immutable terminal result, with catalog publication owned by the session.
use super::*;
use crate::device::preset_workflow::{PresetWorkflowOutcome, RestorationOutcome};
use serde_json::{json, Value};

/// Native delivery describes host I/O only. Even Delivered is not a device ACK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistentDelivery {
    /// No persistent write has begun native delivery.
    NotAttempted,
    /// Native delivery of a persistent write began; its outcome on the device is unconfirmed.
    Attempted,
    /// All reports of the persistent write were delivered natively.
    Delivered,
}

/// Terminal result of a preset workflow, retained by the owner.
///
/// Use `gateway_result` to turn it into the shared public response.
#[derive(Debug, Clone)]
pub struct PresetWorkflowResult {
    /// Final outcome reported by the workflow runtime.
    pub outcome: PresetWorkflowOutcome,
    /// How far native delivery of the persistent write got.
    pub persistent_delivery: PersistentDelivery,
    /// Catalog resolution that verified the mutation, if any.
    pub catalog: Option<PresetCatalogResolution>,
    /// A verified native workflow readback still matching the current source at
    /// termination. Omitted after navigation/session drift or catalog-only proof.
    pub snapshot: Option<crate::device::GatewaySnapshot>,
    detail: String,
    saved_name: String,
    setlist_key: String,
}

impl PresetWorkflowResult {
    pub(super) fn capture(
        entry: &PresetWorkflowEntry,
        outcome: PresetWorkflowOutcome,
        owner: &SessionRuntime,
    ) -> Self {
        let snapshot = entry
            .verified_snapshot
            .as_ref()
            .filter(|(_, source)| {
                owner.accepts_fence(entry.context.session_fence()) && source.matches(&owner.state)
            })
            .map(|(snapshot, _)| snapshot.clone());
        Self {
            outcome,
            persistent_delivery: entry.persistent_delivery,
            catalog: entry.catalog_result.clone(),
            snapshot,
            detail: entry.runtime.plan().detail.clone(),
            saved_name: entry.runtime.plan().saved_name.clone(),
            setlist_key: entry.runtime.plan().setlist_key.clone(),
        }
    }

    /// Shared public result semantics for both native hosts. A cleanup failure
    /// does not erase a catalog-verified mutation or invite a persistent replay.
    pub fn gateway_result(&self) -> Result<Value, String> {
        let (restoration_status, restoration_detail) = match &self.outcome.restoration {
            RestorationOutcome::NotRequired => ("not_required", None),
            RestorationOutcome::Completed => ("completed", None),
            RestorationOutcome::Skipped(reason) => ("skipped", Some(reason.as_str())),
            RestorationOutcome::Failed(reason) => ("failed", Some(reason.as_str())),
        };
        if let Some(primary) = &self.outcome.primary_error {
            let evidence = match self.persistent_delivery {
                PersistentDelivery::NotAttempted => "No persistent USB write was attempted.",
                PersistentDelivery::Attempted => {
                    "A persistent USB write was attempted; its device outcome remains unconfirmed."
                }
                PersistentDelivery::Delivered => {
                    "Persistent USB reports were delivered, but the device outcome was not verified."
                }
            };
            let cleanup = match &self.outcome.restoration {
                RestorationOutcome::NotRequired => String::new(),
                RestorationOutcome::Completed => {
                    " The original preset navigation was restored.".into()
                }
                RestorationOutcome::Skipped(reason) => {
                    format!(" Navigation restoration was skipped: {reason}")
                }
                RestorationOutcome::Failed(reason) => {
                    format!(" Navigation restoration failed: {reason}")
                }
            };
            return Err(format!("{evidence} {primary}{cleanup}"));
        }
        let verified = self.outcome.catalog_verified && self.catalog.is_some();
        let (detail, saved_name, setlist_key) = self.catalog.as_ref().map_or(
            (
                self.detail.as_str(),
                self.saved_name.as_str(),
                self.setlist_key.as_str(),
            ),
            |catalog| {
                (
                    catalog.detail.as_str(),
                    catalog.saved_name.as_str(),
                    catalog.setlist_key.as_str(),
                )
            },
        );
        let detail = match restoration_detail {
            Some(reason) => {
                format!("{detail}. Navigation restoration {restoration_status}: {reason}")
            }
            None => detail.to_string(),
        };
        // The write was delivered but no listing confirmed it inside the bounded window. Say so
        // in the detail instead of the success wording the plan carries ("... and verified").
        let detail = match (&self.outcome.catalog_unconfirmed, verified) {
            (Some(unconfirmed), false) => format!(
                "{} was sent but is not verified: {unconfirmed} Read the library back to see the result; the write was not replayed.",
                self.sent_summary()
            ),
            _ => detail,
        };
        let mut value = json!({
            "accepted": true, "verified": verified,
            "verification": if verified { "authoritative_readback" } else { "accepted_unverified" },
            "detail": detail, "setlistKey": setlist_key,
            "restoration": { "status": restoration_status },
        });
        if !saved_name.is_empty() {
            value["savedName"] = json!(saved_name);
        }
        if let Some(reason) = restoration_detail {
            value["restoration"]["detail"] = json!(reason);
        }
        if let Some(snapshot) = &self.snapshot {
            value["snapshot"] =
                serde_json::to_value(snapshot).map_err(|error| error.to_string())?;
        }
        // A matching catalog name cannot prove a same-name overwrite changed
        // stored Grid contents. Leave changed absent rather than asserting it.
        Ok(value)
    }
}

impl PresetWorkflowResult {
    /// What was sent, without the plan's success wording ("... and verified").
    fn sent_summary(&self) -> String {
        let detail = self.detail.as_str();
        let trimmed = detail
            .strip_suffix(" and verified the fresh catalog")
            .or_else(|| detail.strip_suffix(" and verified the swap"))
            .or_else(|| detail.strip_suffix(" and verified"))
            .unwrap_or(detail);
        let trimmed = trimmed
            .strip_prefix("Saved and verified ")
            .map_or_else(|| trimmed.to_string(), |name| format!("Save of {name}"));
        let trimmed = trimmed
            .strip_prefix("Renamed and verified ")
            .map_or_else(|| trimmed.clone(), |name| format!("Rename to {name}"));
        trimmed
    }
}

impl SessionRuntime {
    /// Returns the finished result of workflow `id`, or `None` while it is still running.
    ///
    /// Fails if `id` is no longer the retained workflow.
    pub fn preset_workflow_result(&self, id: u64) -> Result<Option<&PresetWorkflowResult>, String> {
        let entry = self
            .preset_workflow
            .as_ref()
            .filter(|entry| entry.id == id)
            .ok_or("QC preset workflow identity is no longer active")?;
        Ok(entry.result.as_ref())
    }

    pub(in crate::device::owner) fn preset_workflow_native_started(
        &mut self,
        operation: &OperationTicket,
    ) {
        if let Some(entry) = self.preset_workflow.as_mut().filter(|entry| {
            entry
                .native
                .as_ref()
                .is_some_and(|native| native.operation == *operation && native.persistent)
        }) {
            entry.persistent_delivery = PersistentDelivery::Attempted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_preset_results_satisfy_gateway_contracts_without_claiming_changed_contents() {
        for verified in [false, true] {
            for restoration in [
                RestorationOutcome::NotRequired,
                RestorationOutcome::Completed,
                RestorationOutcome::Failed("Recall failed".into()),
                RestorationOutcome::Skipped("Source changed".into()),
            ] {
                let result = PresetWorkflowResult {
                    outcome: PresetWorkflowOutcome {
                        primary_error: None,
                        catalog_verified: verified,
                        catalog_unconfirmed: None,
                        restoration,
                    },
                    persistent_delivery: PersistentDelivery::Delivered,
                    catalog: verified.then(|| PresetCatalogResolution {
                        setlist_key: "/media/p4/Presets/Created".into(),
                        saved_name: "Created".into(),
                        detail: "Verified catalog".into(),
                        saved_presets: vec![],
                    }),
                    snapshot: None,
                    detail: "Sent".into(),
                    saved_name: "Requested".into(),
                    setlist_key: "/media/p4/Presets/Requested".into(),
                }
                .gateway_result()
                .unwrap();
                assert_eq!(result["verified"], verified);
                assert_eq!(
                    result["setlistKey"],
                    if verified {
                        "/media/p4/Presets/Created"
                    } else {
                        "/media/p4/Presets/Requested"
                    }
                );
                assert!(result.get("changed").is_none());
                assert!(result.get("snapshot").is_none());
                for method in [
                    "device.savePresetAs",
                    "device.renameCurrentPreset",
                    "device.copyPreset",
                    "device.createSetlist",
                    "device.duplicateSetlist",
                    "device.movePreset",
                    "device.deletePreset",
                    "device.deleteSetlist",
                ] {
                    crate::device::generated_gateway::validate_result(method, &result).unwrap();
                    let mut missing_outcome = result.clone();
                    missing_outcome
                        .as_object_mut()
                        .unwrap()
                        .remove("verification");
                    assert!(crate::device::generated_gateway::validate_result(
                        method,
                        &missing_outcome
                    )
                    .is_err());
                }
                let mut missing_key = result;
                missing_key["setlistKey"] = json!("");
                assert!(crate::device::generated_gateway::validate_result(
                    "device.duplicateSetlist",
                    &missing_key
                )
                .is_err());
            }
        }
    }
}
