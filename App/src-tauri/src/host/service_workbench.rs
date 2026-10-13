use super::*;
impl HostCore {
    pub(super) async fn workbench_request(
        self: &Arc<Self>,
        request: &HostRequest,
        owner: &str,
    ) -> Result<Value, HostError> {
        let w = self.workbench.as_ref().map_err(Clone::clone)?.clone();
        let p = request.params.clone();
        match request.method.as_str() {
            "analysis_status" => {
                fields(&p, &["job_id"])?;
                w.visible_status(text_param(&p, "job_id")?, owner)
            }
            "job_cancel" => {
                fields(&p, &["job_id"])?;
                w.cancel(text_param(&p, "job_id")?, owner)
            }
            "analysis_start" => {
                fields(&p, &["request"])?;
                let r: yang_data::AnalysisRequest = serde_json::from_value(p["request"].clone())
                    .map_err(|e| HostError::new("Analysis", e.to_string()))?;
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || w.analysis(&owner, r))
                    .await
                    .map_err(task_error)?
            }
            "experiment_templates" => {
                empty(&p)?;
                Ok(yang_experiments::templates())
            }
            "experiment_start" | "capture_start" => {
                fields(&p, &["definition"])?;
                let d = serde_json::from_value(p["definition"].clone()).map_err(task_error)?;
                self.worker_cache_with_refresh(true).await?;
                let core = self.clone();
                let owner = owner.to_owned();
                let manual = request.method == "capture_start";
                tokio::task::spawn_blocking(move || if manual { experiment_port::capture_start(core, &owner, d) } else { experiment_port::start(core, &owner, d) })
                    .await
                    .map_err(task_error)?
            }
            "experiment_status" => {
                fields(&p, &["run_id"])?;
                let id = text_param(&p, "run_id")?;
                let mut v = w.status(id, owner)?;
                v["run_id"] = json!(id);
                v["record_ids"] = v["result"]["run"]["records"].clone();
                v["execution_id"] = v["result"]["run"]["id"].clone();
                // Older persisted jobs retain experiment failures only in the
                // full RunRecord. Project them without replacing a job error.
                if v["error"].is_null() {
                    v["error"] = v["result"]["run"]["error"].clone();
                }
                v["cleanup_errors"] = v["result"]["run"]["cleanup_errors"].clone();
                Ok(v)
            }
            "experiment_stop" => {
                fields(&p, &["run_id"])?;
                let id = text_param(&p, "run_id")?;
                let mut v = w.cancel(id, owner)?;
                v["run_id"] = json!(id);
                Ok(v)
            }
            "experiment_list" => {
                empty(&p)?;
                tokio::task::spawn_blocking(move || {
                    let mut v = w.definitions()?;
                    v["runs"] = w.runs()?;
                    Ok(v)
                })
                .await
                .map_err(task_error)?
            }
            "experiment_save" => {
                fields(&p, &["definition"])?;
                let d = serde_json::from_value(p["definition"].clone()).map_err(task_error)?;
                tokio::task::spawn_blocking(move || w.save_definition(d))
                    .await
                    .map_err(task_error)?
            }
            "data_import_begin" => {
                fields(&p, &["name", "byte_count"])?;
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || {
                    w.import_begin(
                        &owner,
                        text_param(&p, "name")?,
                        uint_param(&p, "byte_count")?,
                    )
                })
                .await
                .map_err(task_error)?
            }
            "data_import_chunk" => {
                fields(&p, &["transfer_id", "offset", "data_hex"])?;
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || {
                    w.import_chunk(
                        &owner,
                        text_param(&p, "transfer_id")?,
                        uint_param(&p, "offset")?,
                        text_param(&p, "data_hex")?,
                    )
                })
                .await
                .map_err(task_error)?
            }
            "data_import_commit" => {
                fields(&p, &["transfer_id", "append"])?;
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || {
                    w.import_commit(
                        &owner,
                        text_param(&p, "transfer_id")?,
                        p["append"]
                            .as_bool()
                            .ok_or_else(|| task_error("append must be boolean"))?,
                    )
                })
                .await
                .map_err(task_error)?
            }
            "data_import_abort" => {
                fields(&p, &["transfer_id"])?;
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || {
                    w.import_abort(&owner, text_param(&p, "transfer_id")?)
                })
                .await
                .map_err(task_error)?
            }
            "data_export" => {
                optional_fields(&p, &["record_ids", "format"], &["frame_id"])?;
                if !p["frame_id"].is_null() {
                    text_param(&p, "frame_id")?;
                }
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || w.export(&owner, &p))
                    .await
                    .map_err(task_error)?
            }
            "data_export_release" => {
                fields(&p, &["job_id"])?;
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || {
                    w.export_release(&owner, text_param(&p, "job_id")?)
                })
                .await
                .map_err(task_error)?
            }
            "data_export_chunk" => {
                fields(&p, &["job_id", "file_id", "offset", "length"])?;
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || w.export_chunk(&owner, &p))
                    .await
                    .map_err(task_error)?
            }
            "data_list" => {
                empty(&p)?;
                tokio::task::spawn_blocking(move || w.list())
                    .await
                    .map_err(task_error)?
            }
            "data_get" => {
                optional_fields(
                    &p,
                    &["record_id"],
                    &["frame_id", "frame_offset", "frame_limit"],
                )?;
                if !p["frame_id"].is_null() {
                    text_param(&p, "frame_id")?;
                }
                for key in ["frame_offset", "frame_limit"] {
                    if p.get(key).is_some() {
                        uint_param(&p, key)?;
                    }
                }
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || {
                    let id = text_param(&p, "record_id")?;
                    let limit = p["frame_limit"].as_u64().unwrap_or(64).clamp(1, 256) as usize;
                    let offset = if let Some(frame) = p["frame_id"].as_str() {
                        w.record_metadata(&owner, id)
                            .map_err(task_error)?
                            .frames
                            .iter()
                            .position(|f| f.id == frame)
                            .ok_or_else(|| task_error("Frame absent"))?
                            / limit
                            * limit
                    } else {
                        p["frame_offset"].as_u64().unwrap_or(0) as usize
                    };
                    w.owner_metadata(&owner, id, offset, limit)
                })
                .await
                .map_err(task_error)?
            }
            "data_viewport" => {
                fields(
                    &p,
                    &[
                        "record_id",
                        "frame_id",
                        "channel",
                        "start",
                        "end",
                        "max_points",
                    ],
                )?;
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || w.owner_viewport(&owner, &p))
                    .await
                    .map_err(task_error)?
            }
            "preview_save" => {
                fields(&p, &["record_id", "name", "comment"])?;
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || w.preview_save(&owner, text_param(&p, "record_id")?, text_param(&p, "name")?,
                    p["comment"].as_str().ok_or_else(|| task_error("comment must be text"))?))
                    .await.map_err(task_error)?
            }
            "preview_discard" => {
                fields(&p, &["record_id"])?;
                let owner = owner.to_owned();
                tokio::task::spawn_blocking(move || w.preview_discard(&owner, text_param(&p, "record_id")?))
                    .await.map_err(task_error)?
            }
            "analysis_result" => {
                fields(&p, &["record_id", "analysis_id"])?;
                tokio::task::spawn_blocking(move || {
                    let r = w
                        .store
                        .metadata(text_param(&p, "record_id")?)
                        .map_err(task_error)?;
                    let a = r
                        .analyses
                        .iter()
                        .find(|a| Some(a.id.as_str()) == p["analysis_id"].as_str())
                        .ok_or_else(|| task_error("Saved analysis absent"))?;
                    Ok(json!({"analysis":a}))
                })
                .await
                .map_err(task_error)?
            }
            "data_update" => {
                optional_fields(&p, &["record_id", "name"], &["comment"])?;
                tokio::task::spawn_blocking(move || {
                    let id = text_param(&p, "record_id")?;
                    let name = text_param(&p, "name")?;
                    match p.get("comment") {
                        Some(comment) => w.store.rename(id, name,
                            comment.as_str().ok_or_else(|| task_error("comment must be text"))?),
                        None => w.store.rename_name(id, name),
                    }.map_err(task_error)?;
                    Ok(json!({"saved":true}))
                })
                .await
                .map_err(task_error)?
            }
            "data_remove" => {
                fields(&p, &["record_id"])?;
                tokio::task::spawn_blocking(move || {
                    w.store
                        .remove(text_param(&p, "record_id")?)
                        .map_err(task_error)?;
                    Ok(json!({"removed":true,"recoverable":true}))
                })
                .await
                .map_err(task_error)?
            }
            _ => Err(task_error("Unknown workbench method")),
        }
    }
}
fn task_error(e: impl std::fmt::Display) -> HostError {
    HostError::new("Workbench", e.to_string())
}
fn optional_fields(p: &Value, required: &[&str], optional: &[&str]) -> Result<(), HostError> {
    if p.as_object().is_some_and(|o| {
        required.iter().all(|k| o.contains_key(*k))
            && o.keys()
                .all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str()))
    }) {
        Ok(())
    } else {
        Err(HostError::new(
            "HostProtocol",
            "Unexpected or missing parameters",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_record_name_only_update_preserves_latest_comment_and_analysis() {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
            let fixture = super::super::native_osa_flow_tests::Fixture::new(true);
            let store = &fixture.core().workbench.as_ref().unwrap().store;
            let record: yang_data::WaveformRecord = serde_json::from_value(json!({
                "schema":1,"id":"1".repeat(32),"name":"Earlier scope name","comment":"Earlier scope comment","created_at_unix_ms":1,
                "source":{"kind":"manual"},"analyses":[],"frames":[{"id":"2".repeat(32),"captured_at_unix_ms":2,"nominal_offset_s":null,
                "channels":[{"channel":"CH1","samples":{"u16":[0,100,65535]},"calibration":{"x_increment_s":0.000001,"x_origin_s":0.0,"x_reference":0.0,"y_increment_v":0.01,"y_origin":0.0,"y_reference":0.0},"metadata":{}}]}]
            })).unwrap();
            store.save(&record).unwrap();
            // Record/Data has changed fields which the Scope name-only form does not own.
            store.rename(&record.id, "Record/Data name", "Newest Record/Data comment").unwrap();
            let analysis: yang_data::AnalysisResult = serde_json::from_value(json!({
                "schema":1,"id":"3".repeat(32),"record_id":record.id,"frame_id":"2".repeat(32),"channel":"CH1",
                "roi":{"start":0,"end":3},"method":"fsr","algorithm":"fixture-v1","parameters":{},"result":{"fsr_nm":1.0},
                "curve":[],"markers":[],"created_at_unix_ms":3
            })).unwrap();
            store.append_analysis(analysis.clone()).unwrap();
            let request = HostRequest { v: 1, id: new_id().unwrap(), method: "data_update".into(),
                params: json!({"record_id":record.id,"name":"New Scope name"}) };
            let result = fixture.core().dispatch(&request, fixture.local.session.id()).await;
            assert!(result.is_ok(), "Name-only update must accept an omitted comment: {result:?}");
            let latest = store.metadata(&record.id).unwrap();
            assert_eq!(latest.name, "New Scope name");
            assert_eq!(latest.comment, "Newest Record/Data comment");
            assert_eq!(latest.analyses, vec![analysis]);
            let mut malformed = request.clone();
            malformed.params["name"] = json!("Must not be applied");
            malformed.params["comment"] = Value::Null;
            assert!(fixture.core().dispatch(&malformed, fixture.local.session.id()).await.is_err());
            assert_eq!(store.metadata(&record.id).unwrap().name, "New Scope name");
        });
    }
}
