use super::*;

impl Graphs {
    /// The open pull requests as last listed, and listed now when they never
    /// were: what the current work is measured against must not depend on
    /// which request reaches the server first.
    pub(super) fn listed(&self, project: &str, root: &Path) -> Vec<Pull> {
        if let Ok(pulls) = self.pulls.lock()
            && let Some((_, pulls, _)) = pulls.get(project)
        {
            return pulls.clone();
        }
        let listed = graph_changes::pulls(root);
        if let Ok(mut pulls) = self.pulls.lock() {
            pulls.insert(
                project.to_owned(),
                (Instant::now(), listed.0.clone(), listed.1.clone()),
            );
        }
        listed.0
    }
    /// What the checkout is on and the open pull requests of its repository.
    /// Asking the forge takes a moment, so callers keep this off the runtime.
    pub fn pulls(&self, project: &str) -> Result<Value> {
        let root = self.root(project)?;
        let mut place = graph_changes::place(&root)?;
        if place["git"] != true {
            return Ok(place);
        }
        let known = lock(&self.pulls)?
            .get(project)
            .filter(|(at, ..)| at.elapsed() < PULLS_AGE)
            .map(|(_, pulls, reason)| (pulls.clone(), reason.clone()));
        let (pulls, reason) = match known {
            Some(known) => known,
            None => {
                let listed = graph_changes::pulls(&root);
                lock(&self.pulls)?.insert(
                    project.to_owned(),
                    (Instant::now(), listed.0.clone(), listed.1.clone()),
                );
                listed
            }
        };
        // The request the current branch belongs to names what it is stacked on.
        let own = pulls
            .iter()
            .find(|pull| place["branch"].as_str() == Some(pull.head.as_str()));
        place["pull"] = json!(own.map(|pull| pull.number));
        place["against"] = json!(own.map(|pull| pull.base.clone()));
        place["pulls"] = pulls.iter().map(Pull::json).collect();
        place["reason"] = json!(reason);
        Ok(place)
    }
    /// Pull requests in one state, narrowed by the forge's own search, for
    /// the picker. The forge is asked again after a minute.
    pub fn found(&self, project: &str, state: &str, query: &str) -> Result<Value> {
        let root = self.root(project)?;
        let key = (project.to_owned(), state.to_owned(), query.to_owned());
        let known = lock(&self.searches)?
            .get(&key)
            .filter(|(at, ..)| at.elapsed() < PULLS_AGE)
            .map(|(_, pulls, reason)| (pulls.clone(), reason.clone()));
        let (pulls, reason) = match known {
            Some(known) => known,
            None => {
                let listed = graph_changes::search(&root, state, query);
                let mut searches = lock(&self.searches)?;
                searches.retain(|_, (at, ..)| at.elapsed() < PULLS_AGE);
                searches.insert(key, (Instant::now(), listed.0.clone(), listed.1.clone()));
                listed
            }
        };
        Ok(json!({
            "pulls": pulls.iter().map(Pull::json).collect::<Vec<_>>(),
            "reason": reason,
        }))
    }
    /// A page of the project's history for the picker.
    pub fn commits(&self, project: &str, scope: &str, query: &str, skip: usize) -> Result<Value> {
        let (commits, more) = graph_changes::log(&self.root(project)?, scope, query, skip)?;
        Ok(json!({"commits": commits, "more": more, "skip": skip}))
    }
    /// The project's branches for the picker.
    pub fn branches(&self, project: &str) -> Result<Value> {
        let root = self.root(project)?;
        Ok(json!({
            "branches": graph_changes::branches(&root, &self.listed(project, &root))?,
        }))
    }
    /// How long builds of this project take, for the estimate of a commit's
    /// graph: the last such build, else the project's own, which runs the
    /// same passes over nearly the same files.
    pub(super) fn pace(&self, setting: &Value) -> Option<History> {
        let version = json!(self.resolve_backend(false).and_then(|b| b.tool_version()));
        [&setting["revision"], &setting["built"]]
            .into_iter()
            .find(|built| built["tool"] == version && built["seconds"].is_number())
            .and_then(|built| {
                Some(History {
                    seconds: built["seconds"].as_f64()?,
                    tail: built["tail"].as_f64(),
                })
            })
    }
    /// Whether the project's own graph is of this commit: it is when it was
    /// built from a clean checkout of it.
    pub(super) fn current(&self, project: &str, setting: &Value, commit: &str) -> bool {
        live(&self.store.state, project).is_file()
            && setting["built"]["print"].as_str()
                == Some(util::sha(format!("{commit}\0").as_bytes()).as_str())
    }
    pub(super) fn graph_of(
        &self,
        project: &str,
        root: &Path,
        resolved: &Resolved,
    ) -> Result<Found> {
        self.graph_at(
            project,
            root,
            resolved.revision.as_deref(),
            &resolved.prefix,
            false,
        )
    }
    pub(super) fn graph_at(
        &self,
        project: &str,
        root: &Path,
        commit: Option<&str>,
        prefix: &str,
        exact: bool,
    ) -> Result<Found> {
        let ready = |kept: String, graph: PathBuf, base: PathBuf| Found {
            state: "ready",
            build: Value::Null,
            failure: None,
            graph: Some((kept, graph, base)),
        };
        let published = live(&self.store.state, project);
        let Some(commit) = commit else {
            return Ok(ready(project.to_owned(), published, root.to_owned()));
        };
        let own = revision(&self.store.state, project, commit);
        if own.is_file() {
            let tree = own.with_file_name("tree");
            return Ok(ready(
                format!("{project}@{commit}"),
                own,
                if prefix.is_empty() {
                    tree
                } else {
                    tree.join(prefix)
                },
            ));
        }
        let setting = self.setting(project)?;
        if !exact && self.current(project, &setting, commit) {
            return Ok(ready(project.to_owned(), published, root.to_owned()));
        }
        let waiting = |state, build, failure| Found {
            state,
            build,
            failure,
            graph: None,
        };
        if let Some((building, job)) = lock(&self.revisions)?.get(project)
            && building == commit
        {
            return Ok(waiting("building", job.view(self.pace(&setting)), None));
        }
        Ok(
            match lock(&self.refused)?.get(&(project.to_owned(), commit.to_owned())) {
                Some(message) => waiting("failed", Value::Null, Some(message.clone())),
                None => waiting("missing", Value::Null, None),
            },
        )
    }
    /// Reads what a run did to the files of its project from the steps its
    /// session recorded, as the run's own receipt counts them: the files it
    /// changed, each with what was done to it, and the files it opened. A
    /// session of another project is no run of this one.
    pub(super) fn ran(&self, project: &str, root: &Path, session: &str) -> Result<RunChange> {
        Ok(self.run_launches(project, root, session, None, None)?.0)
    }

    pub(super) fn run_launches(
        &self,
        project: &str,
        root: &Path,
        session: &str,
        launch: Option<&str>,
        compare: Option<&str>,
    ) -> Result<(RunChange, Option<RunChange>)> {
        let record = match self.runs.load_run(root, session)? {
            Some(found) => found,
            None => {
                let sess = self
                    .store
                    .get(session)
                    .map_err(|_| GraphError::missing("This project has no such run."))?;
                if sess["project_id"] != project {
                    return Err(GraphError::missing("This project has no such run."));
                }
                let events = self.store.events(session, 0)?;
                let ws = PathBuf::from(sess["project_path"].as_str().unwrap_or(""));
                crate::run_activity::record_from_events(
                    session.to_owned(),
                    sess["title"].as_str().unwrap_or("").to_owned(),
                    sess["provider"].as_str().unwrap_or("").to_owned(),
                    sess["status"].as_str().unwrap_or("").to_owned(),
                    ws,
                    &events,
                )
            }
        };
        let mut known = BTreeSet::new();
        let mut launches = Vec::new();
        for item in &record.launches {
            if item.id.is_empty() {
                continue;
            }
            let number = launches.len() + 1;
            known.insert(item.id.clone());
            launches.push(json!({
                "id": item.id,
                "number": number,
                "at": item.at,
            }));
        }
        if launch
            .into_iter()
            .chain(compare)
            .any(|id| !known.contains(id))
        {
            return Err(GraphError::missing(
                "This session has no such recorded launch.",
            ));
        }

        let merge_launches = |filter_id: Option<&str>| -> crate::run_activity::LaunchRecord {
            let mut merged = crate::run_activity::LaunchRecord {
                id: filter_id.unwrap_or("all").to_owned(),
                at: None,
                steps: 0,
                opened: BTreeSet::new(),
                edited: Vec::new(),
                shell_reads: BTreeMap::new(),
                shell_commands: BTreeMap::new(),
                outside_count: 0,
                searched: 0,
                commands: 0,
                graph_queries: 0,
                helpers: 0,
                limited: false,
            };
            let mut edited_index: BTreeMap<String, usize> = BTreeMap::new();
            for item in &record.launches {
                if let Some(wanted) = filter_id
                    && item.id != wanted
                {
                    continue;
                }
                merged.steps += item.steps;
                merged.opened.extend(item.opened.iter().cloned());
                for (path, status) in &item.edited {
                    if let Some(&pos) = edited_index.get(path) {
                        merged.edited[pos].1 = *status;
                    } else {
                        edited_index.insert(path.clone(), merged.edited.len());
                        merged.edited.push((path.clone(), *status));
                    }
                }
                for (path, ev_id) in &item.shell_reads {
                    merged.shell_reads.insert(path.clone(), *ev_id);
                }
                merged.outside_count += item.outside_count;
                merged.searched += item.searched;
                merged.commands += item.commands;
                merged.graph_queries += item.graph_queries;
                merged.helpers += item.helpers;
                merged.limited |= item.limited;
            }
            merged
        };

        let mut primary = Self::run_change(root, &record, merge_launches(launch));
        primary.told["launches"] = json!(launches);
        primary.told["launch"] = json!(launch);
        primary.told["compare_launch"] = json!(compare);
        let _ = project;
        Ok((
            primary,
            compare.map(|cmp| Self::run_change(root, &record, merge_launches(Some(cmp)))),
        ))
    }

    pub(super) fn run_change(
        root: &Path,
        record: &crate::run_activity::RecordedRun,
        launch: crate::run_activity::LaunchRecord,
    ) -> RunChange {
        let plain = |path: &str| path.replace('\\', "/").trim_start_matches("./").to_owned();
        let opened: BTreeSet<String> = launch
            .opened
            .iter()
            .map(|p| plain(p))
            .chain(launch.shell_reads.keys().map(|p| plain(p)))
            .collect();
        let files = launch
            .edited
            .iter()
            .map(|(path, status)| graph_changes::Change {
                path: plain(path),
                status: *status,
                from: None,
            })
            .collect::<Vec<_>>();
        let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let canonical_ws =
            std::fs::canonicalize(&record.project_path).unwrap_or_else(|_| record.project_path.clone());
        let elsewhere = !record.project_path.as_os_str().is_empty()
            && canonical_ws != canonical_root
            && record.project_path != root;
        let told = json!({
            "session": record.session_id,
            "title": record.title,
            "provider": record.provider,
            "status": record.status,
            "steps": launch.steps,
            "opened": opened.len(),
            "edited": files.len(),
            "shell_opened": launch.shell_reads.len(),
            "outside": launch.outside_count,
            "searched": launch.searched,
            "commands": launch.commands,
            "helpers": launch.helpers,
            "limited": launch.limited,
            "elsewhere": elsewhere,
        });
        RunChange {
            resolved: Arc::new(Resolved {
                target: Target::Run(record.session_id.clone()),
                kind: "run",
                name: record.title.chars().take(80).collect(),
                title: None,
                pull: None,
                head: None,
                base: None,
                revision: None,
                prefix: String::new(),
                files,
                notes: Vec::new(),
            }),
            opened,
            shell_reads: launch.shell_reads,
            told,
        }
    }
    /// Says in a picture of a folder's layers what the chosen change did
    /// to their order: the pairs of files that run against it now and did
    /// not where the change started, and those that did and no longer do.
    /// The graph of that base is one of the cached graphs of commits. Until
    /// it is built the picture says that it is being prepared, and where
    /// the base cannot be told or its graph cannot be built, that the
    /// comparison is not to be had.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn compare_layers(
        &self,
        project: &str,
        root: &Path,
        resolved: &Resolved,
        head: &crate::graph_map::Index,
        view: &mut Value,
        shown: &Shown,
        rules: Option<&crate::graph_map::layer_rules::Rules>,
    ) {
        let base_side = resolved.json()["base"].clone();
        let compare = |view: &mut Value| -> Result<Value> {
            let Some(base) = &resolved.base else {
                return Ok(json!({"state": "unavailable", "reason": "no_base"}));
            };
            let found =
                self.graph_at(project, root, Some(&base.commit), &resolved.prefix, false)?;
            let Some((key, graph, tree)) = found.graph else {
                if found.state == "failed" {
                    return Ok(json!({"state": "unavailable"}));
                }
                if found.state == "missing" {
                    // Beside the graph of a revision the base is kept as
                    // its pair; the checkout's own work has no revision.
                    self.revise(
                        project,
                        root,
                        &base.commit,
                        &resolved.prefix,
                        match &resolved.revision {
                            Some(head) => RevisionBuild::Comparison { head },
                            None => RevisionBuild::Normal,
                        },
                    )?;
                }
                return Ok(json!({"state": "building"}));
            };
            let base = self.maps.index(&key, &graph, &tree)?;
            // The project's own graph, where it stands for the commit, is
            // read with the files of the checkout; the graph of a commit
            // tells its tests by the spans kept beside it.
            if shown.notests
                && key.contains('@')
                && base.has_rust_sources()
                && !base.has_test_spans()
            {
                return Ok(json!({"state": "unavailable", "reason": "tests"}));
            }
            let base = if shown.notests {
                base.without_tests()
            } else {
                base
            };
            let renamed = resolved
                .files
                .iter()
                .filter_map(|file| {
                    (file.status == 'R').then_some((file.path.as_str(), file.from.as_deref()?))
                })
                .collect();
            if view["pattern"]["declared"] == true {
                let Some(rules) = rules else {
                    return Ok(json!({"state": "unavailable", "reason": "no_rules"}));
                };
                head.compare_declared_layers(&base, view, &renamed, shown.certain, rules);
            } else {
                head.compare_layers(&base, view, &renamed, shown.certain);
            }
            Ok(view["pattern"]["change"].take())
        };
        let mut change = compare(view).unwrap_or_else(|_| json!({"state": "unavailable"}));
        change["base"] = base_side;
        view["pattern"]["change"] = change;
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn compare_pr_cycle(
        &self,
        project: &str,
        root: &Path,
        resolved: &Resolved,
        raw_head: &crate::graph_map::Index,
        head: &crate::graph_map::Index,
        snapshot: bool,
        view: &mut Value,
        shown: &Shown,
    ) {
        let base_side = resolved.json()["base"].clone();
        let mut compare = || -> Result<Value> {
            let (Some(base), Some(head_commit)) = (&resolved.base, &resolved.revision) else {
                return Ok(json!({"state": "unavailable", "reason": "no_base"}));
            };
            // Keep ordinary PR views able to reuse the matching live graph.
            // Comparison needs a snapshot so its source filters describe
            // the revision even if the checkout subsequently changes.
            if !snapshot {
                let found =
                    self.graph_at(project, root, Some(head_commit), &resolved.prefix, true)?;
                if found.state == "failed" {
                    return Ok(
                        json!({"state": "unavailable", "reason": "head", "base": base_side}),
                    );
                }
                if found.state == "missing" {
                    self.revise(
                        project,
                        root,
                        head_commit,
                        &resolved.prefix,
                        RevisionBuild::Comparison { head: head_commit },
                    )?;
                }
                return Ok(json!({"state": "waiting_head", "base": base_side}));
            }
            let revision = &base.commit;
            let found = self.graph_at(project, root, Some(revision), &resolved.prefix, true)?;
            let Some((key, graph, tree)) = found.graph else {
                if found.state == "failed" {
                    return Ok(json!({"state": "unavailable", "base": base_side}));
                }
                if found.state == "missing" {
                    self.revise(
                        project,
                        root,
                        &base.commit,
                        &resolved.prefix,
                        RevisionBuild::Comparison { head: head_commit },
                    )?;
                }
                return Ok(json!({"state": "building", "base": base_side}));
            };
            let base = self.maps.index(&key, &graph, &tree)?;
            if shown.notests
                && ((snapshot && raw_head.has_rust_sources() && !raw_head.has_test_spans())
                    || (base.has_rust_sources() && !base.has_test_spans()))
            {
                return Ok(json!({"state": "unavailable", "reason": "tests", "base": base_side}));
            }
            let base = if shown.notests {
                base.without_tests()
            } else {
                base
            };
            let renames = resolved
                .files
                .iter()
                .filter_map(|file| {
                    (file.status == 'R').then_some((file.path.as_str(), file.from.as_deref()?))
                })
                .collect();
            let (origin, place) =
                head.compare_cycle(raw_head, &base, view, &renames, shown.certain);
            Ok(json!({
                "state": "ready", "base": base_side, "origin": origin, "place": place,
                "revision": revision,
            }))
        };
        let result =
            compare().unwrap_or_else(|_| json!({"state": "unavailable", "base": base_side}));
        view["cycle"]["comparison"] = result;
    }
    /// Works out what a target changed and, for a commit that is not the
    /// checkout, starts building its graph. A pull request is looked up and
    /// fetched here, so callers keep this off the runtime.
    pub fn choose(&self, project: &str, target: &Target) -> Result<Value> {
        let root = self.root(project)?;
        let resolved = match target {
            Target::Run(session) => self.ran(project, &root, session)?.resolved,
            Target::SysRun(_) => {
                return Err(GraphError::missing("System runs require harness runtime."));
            }
            _ => Arc::new(graph_changes::resolve(
                &root,
                target,
                &self.listed(project, &root),
            )?),
        };
        self.insert_chosen((project.to_owned(), target.text()), resolved.clone())?;
        if let Some(commit) = &resolved.revision {
            self.revise(
                project,
                &root,
                commit,
                &resolved.prefix,
                RevisionBuild::Normal,
            )?;
        }
        let found = self.graph_of(project, &root, &resolved)?;
        let message = revision_message(found.state, &found.build, found.failure.as_deref());
        Ok(json!({
            "project_id": project,
            "target": target.text(),
            "chosen": chosen(&resolved),
            "state": found.state,
            "message": message,
            "build": found.build,
            "failure": found.failure,
        }))
    }
    /// Starts the graph of a commit unless it exists or is being built. A
    /// build for another commit of the project gives way: the last one asked
    /// for is the one being looked at.
    pub(super) fn revise(
        &self,
        project: &str,
        root: &Path,
        commit: &str,
        prefix: &str,
        build: RevisionBuild<'_>,
    ) -> Result<()> {
        if commit.len() < 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(GraphError::invalid("That is not a commit."));
        }
        if self.stopping.load(Ordering::Acquire)
            || revision(&self.store.state, project, commit).is_file()
            || (matches!(build, RevisionBuild::Normal)
                && self.current(project, &self.setting(project)?, commit))
        {
            return Ok(());
        }
        let backend = self.resolve_backend(true).ok_or_else(|| {
            GraphError::unavailable(
                "Graphify is not installed. Install it to build the graph of that commit.",
            )
        })?;
        let Some(me) = self.me.upgrade() else {
            return Ok(());
        };
        let job = {
            let mut job = Job::new();
            job.stage.store(WAITING, Ordering::Release);
            let mut revisions = lock(&self.revisions)?;
            if revision(&self.store.state, project, commit).is_file() {
                return Ok(());
            }
            if let Some((building, running)) = revisions.get(project) {
                if building == commit || matches!(build, RevisionBuild::Comparison { .. }) {
                    return Ok(());
                }
                job.revision_gate = running.revision_gate.clone();
                running.cancel.store(true, Ordering::Release);
            }
            let job = Arc::new(job);
            revisions.insert(project.to_owned(), (commit.to_owned(), job.clone()));
            job
        };
        lock(&self.refused)?.remove(&(project.to_owned(), commit.to_owned()));
        let (name, root, commit, prefix) = (
            project.to_owned(),
            root.to_owned(),
            commit.to_owned(),
            prefix.to_owned(),
        );
        let running = job.clone();
        let paired = match build {
            RevisionBuild::Comparison { head } => Some(head.to_owned()),
            _ => None,
        };
        let worker = std::thread::Builder::new()
            .name("harness-graph-revision".into())
            .spawn(move || {
                let _gate = running
                    .revision_gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let outcome = me.grow(&name, &root, &backend, &commit, &prefix, &running);
                me.finish(
                    &name,
                    &commit,
                    &running,
                    outcome,
                    &backend,
                    paired.as_deref(),
                );
            });
        match worker {
            Ok(handle) => {
                let mut workers = lock(&self.workers)?;
                workers.retain(|worker| !worker.is_finished());
                workers.push(handle);
                Ok(())
            }
            Err(_) => {
                let mut revisions = lock(&self.revisions)?;
                if revisions
                    .get(project)
                    .is_some_and(|(_, running)| Arc::ptr_eq(running, &job))
                {
                    revisions.remove(project);
                }
                Err(GraphError::unavailable(
                    "The code graph build could not start.",
                ))
            }
        }
    }
    pub(super) fn run_native_revision(
        &self,
        source: &Path,
        out: &Path,
        job: &Arc<Job>,
    ) -> std::result::Result<Ran, Outcome> {
        let stopped =
            || job.cancel.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire);
        if stopped() {
            return Err(Outcome::Cancelled);
        }
        let db_path = out.join("graph_ir.sqlite3");
        let ir = crate::graph_ir::GraphIrStore::open_at(&db_path)
            .map_err(|err| Outcome::Failed(err.message))?;
        let stats = ir
            .sync_workspace(source)
            .map_err(|err| Outcome::Failed(err.message))?;
        if stopped() {
            return Err(Outcome::Cancelled);
        }
        job.observe(
            &format!(
                "  AST extraction: {}/{} uncached files (100%)",
                stats.files_scanned, stats.files_scanned
            ),
            false,
        );
        let fresh = out.join("graph.json");
        let (nodes, edges) = ir
            .write_live_graph_atomic(&fresh)
            .map_err(|err| Outcome::Failed(err.message))?;
        drop(ir);
        let bytes = std::fs::metadata(&fresh).map_or(0, |m| m.len());
        if stopped() {
            return Err(Outcome::Cancelled);
        }
        Ok(Ran {
            fresh,
            elapsed: job.started().elapsed().as_secs_f64(),
            read: None,
            built: Some((nodes, edges)),
            bytes,
        })
    }
    /// Builds the graph of one commit: its files are written out beside the
    /// project's graph, the tool reads them as it reads the project, and of
    /// them only what the graph is read with is kept: see `thin`.
    pub(super) fn grow(
        &self,
        project: &str,
        root: &Path,
        backend: &BuildBackend,
        commit: &str,
        prefix: &str,
        job: &Arc<Job>,
    ) -> Outcome {
        let stopped =
            || job.cancel.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire);
        // One graph is built for a project at a time: two runs of the tool
        // take twice the memory.
        loop {
            if stopped() {
                return Outcome::Cancelled;
            }
            match self.jobs.lock() {
                Ok(jobs) if !jobs.contains_key(project) => break,
                _ => std::thread::sleep(Duration::from_millis(200)),
            }
        }
        let own = revision(&self.store.state, project, commit);
        if own.is_file() {
            return Outcome::Cancelled;
        }
        let home = own.with_file_name("");
        let (tree, out) = (own.with_file_name("tree"), own.with_file_name("work"));
        let _ = std::fs::remove_dir_all(&home);
        if std::fs::create_dir_all(&out).is_err()
            || crate::util::private_directory(&folder(&self.store.state, project)).is_err()
        {
            return Outcome::Failed("The graph folder could not be prepared.".into());
        }
        job.stage.store(EXPORTING, Ordering::Release);
        let outcome = (|| {
            if graph_changes::export(root, commit, &tree).is_err() {
                return Outcome::Failed("Git could not write out the files of that commit.".into());
            }
            let source = if prefix.is_empty() {
                tree.clone()
            } else {
                tree.join(prefix)
            };
            if !source.is_dir() {
                return Outcome::Failed("That commit does not have this project's folder.".into());
            }
            if stopped() {
                return Outcome::Cancelled;
            }
            job.restart();
            job.stage.store(0, Ordering::Release);
            let ran = match match backend {
                BuildBackend::Graphify(tool) => self.run(tool, &source, &out, job),
                BuildBackend::NativeIr => self.run_native_revision(&source, &out, job),
            } {
                Ok(ran) => ran,
                Err(outcome) => return outcome,
            };
            if let Some(tests) = thin(&tree) {
                let prefix = if prefix.is_empty() {
                    String::new()
                } else {
                    format!("{prefix}/")
                };
                let spans = tests
                    .iter()
                    .filter_map(|(path, spans)| Some((path.strip_prefix(&prefix)?, spans)))
                    .collect::<BTreeMap<_, _>>();
                if let Ok(spans) = serde_json::to_vec(&spans) {
                    let file = own.with_file_name(crate::graph_map::TEST_SPANS);
                    if std::fs::write(&file, spans).is_err() {
                        let _ = std::fs::remove_file(file);
                    }
                }
            }
            if stopped() {
                return Outcome::Cancelled;
            }
            if std::fs::rename(&ran.fresh, &own).is_err() {
                return Outcome::Failed("The new graph could not be published.".into());
            }
            Outcome::Built(ran.record(ran.bytes))
        })();
        // The files and the tool's cache are as large as the project. What
        // the graph is read with is not.
        let _ = std::fs::remove_dir_all(&out);
        if !matches!(outcome, Outcome::Built(_)) {
            let _ = std::fs::remove_dir_all(&home);
        }
        outcome
    }
    pub(super) fn finish(
        &self,
        project: &str,
        commit: &str,
        job: &Arc<Job>,
        outcome: Outcome,
        backend: &BuildBackend,
        paired: Option<&str>,
    ) {
        let purge = job.purge.load(Ordering::Acquire);
        if purge {
            self.stop_watcher(project);
            self.evict_ir_store(project);
            let _ = std::fs::remove_dir_all(folder(&self.store.state, project));
        }
        match outcome {
            Outcome::Built(built) if !purge => {
                let _ = self.edit(project, |setting| {
                    setting["revision"] = json!({
                        "seconds": built["seconds"],
                        "tail": built["tail"],
                        "tool": backend.tool_version(),
                    });
                });
                self.prune(project, commit, paired);
            }
            Outcome::Failed(message) => {
                if let Ok(mut refused) = self.refused.lock() {
                    refused.insert((project.to_owned(), commit.to_owned()), message);
                }
            }
            _ => {}
        }
        if let Ok(mut revisions) = self.revisions.lock()
            && revisions
                .get(project)
                .is_some_and(|(_, running)| Arc::ptr_eq(running, job))
        {
            revisions.remove(project);
        }
        // The graph was deleted while this build was ending: the folder
        // that was left to it goes now.
        if !purge && job.purge.load(Ordering::Acquire) {
            self.stop_watcher(project);
            self.evict_ir_store(project);
            let _ = std::fs::remove_dir_all(folder(&self.store.state, project));
        }
    }
    /// Keeps the newest graphs of other commits and removes the rest.
    pub(super) fn prune(&self, project: &str, keep: &str, paired: Option<&str>) {
        let home = folder(&self.store.state, project).join("revisions");
        let Ok(entries) = std::fs::read_dir(&home) else {
            return;
        };
        let mut built = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let changed = std::fs::metadata(entry.path().join("graph.json"))
                    .and_then(|meta| meta.modified())
                    .ok()?;
                Some((name == keep || Some(name.as_str()) == paired, changed, name))
            })
            .collect::<Vec<_>>();
        built.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        for (.., name) in built.into_iter().skip(REVISIONS) {
            let _ = std::fs::remove_dir_all(home.join(&name));
        }
    }
    /// The state the interface draws. `visit` also considers a rebuild, as when
    /// the project is opened.
    pub fn status(&self, project: &str, visit: bool) -> Result<Value> {
        let root = self.root(project)?;
        if visit {
            self.refresh(project, Trigger::Visit)?;
        }
        let setting = self.setting(project)?;
        let enabled = setting["enabled"] == true;
        let backend = self.resolve_backend(false);
        let tool = match &backend {
            Some(BuildBackend::Graphify(tool)) => Some(tool.clone()),
            _ => None,
        };
        let job = lock(&self.jobs)?.get(project).cloned();
        let built = &setting["built"];
        let published = built.is_object() && live(&self.store.state, project).is_file();
        let expected_version = backend.as_ref().and_then(BuildBackend::tool_version);
        let history = built["seconds"]
            .as_f64()
            .filter(|_| published && built["tool"] == json!(expected_version))
            .map(|seconds| History {
                seconds,
                tail: built["tail"].as_f64(),
            });
        let stale = enabled
            && job.is_none()
            && published
            && self
                .print(project, &root, false)
                .is_some_and(|now| built["print"].as_str() != Some(now.hash.as_str()));
        let state = if !enabled {
            "off"
        } else if job.is_some() {
            "building"
        } else if backend.is_none() {
            "missing"
        } else if setting["failure"].is_object() {
            "failed"
        } else if !published {
            "empty"
        } else if stale {
            "stale"
        } else {
            "ready"
        };
        let build = job.map(|job| job.view(history));
        let failure = if setting["failure"].is_object() {
            Some(json!({"message": setting["failure"]["message"], "at": setting["failure"]["at"]}))
        } else {
            None
        };
        let published_native =
            published && built["tool"].as_str() == Some(crate::graph_ir::NATIVE_IR_TOOL);
        let message = status_message_for_tool(
            state,
            build.as_ref(),
            failure.as_ref().and_then(|value| value["message"].as_str()),
            published_native.then_some(crate::graph_ir::NATIVE_IR_TOOL),
        );
        let tool_json = match &backend {
            Some(BuildBackend::Graphify(tool)) => json!({
                "available": true,
                "version": tool.version,
                "path": tool.path.to_string_lossy().into_owned(),
            }),
            Some(BuildBackend::NativeIr) => json!({
                "available": true,
                "backend": crate::graph_ir::NATIVE_IR_TOOL,
                "version": crate::graph_ir::NATIVE_IR_TOOL,
                "path": Value::Null,
            }),
            None => json!({
                "available": false,
                "version": Value::Null,
                "path": Value::Null,
            }),
        };
        let ir_summary = published_native
            .then(|| {
                self.maps.ir_summary_for_revision(
                    &self.store.state,
                    project,
                    self.ir_revision_of(project),
                )
            })
            .flatten();
        let graph_json = if published {
            let mut g = json!({
                "commit": built["commit"],
                "built_at": built["at"],
                "nodes": built["nodes"],
                "edges": built["edges"],
                "bytes": built["bytes"],
                "disk": built["disk"],
                "seconds": built["seconds"],
            });
            if published_native {
                g["backend"] = json!(crate::graph_ir::NATIVE_IR_TOOL);
                if let Some(summary) = &ir_summary {
                    g["ir_summary"] = serde_json::to_value(summary).unwrap_or(Value::Null);
                }
            }
            g
        } else {
            Value::Null
        };
        let rev = self.ir_revision_of(project);
        let mut response = json!({
            "project_id": project,
            "enabled": enabled,
            // Whether launches are told about the graph: a choice of its own.
            "agents": setting["agents"] == true,
            "state": state,
            "revision": rev,
            "ir_revision": rev,
            "message": message,
            "tool": tool_json,
            "graph": graph_json,
            "build": build,
            "failure": failure,
        });
        if published_native {
            response["backend"] = json!(crate::graph_ir::NATIVE_IR_TOOL);
        }
        let _ = tool;
        Ok(response)
    }
    pub fn act(&self, project: &str, action: &str) -> Result<Value> {
        self.root(project)?;
        match action {
            "enable" => {
                self.edit(project, |setting| {
                    setting["enabled"] = json!(true);
                    if let Some(fields) = setting.as_object_mut() {
                        fields.remove("disabled_by_user");
                    }
                })?;
                self.refresh(project, Trigger::Enabled)?;
            }
            "disable" => {
                self.stop_watcher(project);
                self.edit(project, |setting| {
                    setting["enabled"] = json!(false);
                    setting["disabled_by_user"] = json!(true);
                })?;
                if let Some(job) = lock(&self.jobs)?.get(project) {
                    job.cancel.store(true, Ordering::Release);
                }
            }
            // Telling agents about the graph is asked for on its own: the
            // map needs none of it, and it costs tokens on every launch.
            "offer" => {
                if self.setting(project)?["enabled"] != true {
                    return Err(GraphError::invalid(
                        "Turn the code graph on before telling agents about it.",
                    ));
                }
                self.edit(project, |setting| setting["agents"] = json!(true))?;
            }
            "withhold" => {
                self.edit(project, |setting| setting["agents"] = json!(false))?;
            }
            "rebuild" => {
                if self.setting(project)?["enabled"] != true {
                    return Err(GraphError::invalid(
                        "Turn the code graph on before rebuilding it.",
                    ));
                }
                if self.resolve_backend(true).is_none() {
                    return Err(GraphError::unavailable(
                        "Graphify is not installed. Install it, then rebuild.",
                    ));
                }
                self.edit(project, |setting| {
                    if let Some(fields) = setting.as_object_mut() {
                        fields.remove("failure");
                    }
                })?;
                self.refresh(project, Trigger::Manual)?;
            }
            "delete" => {
                self.stop_watcher(project);
                self.evict_ir_store(project);
                self.edit(project, |setting| {
                    let target_root = setting.get("target_root").cloned();
                    let target_name = setting.get("target_name").cloned();
                    *setting = json!({"enabled": false, "disabled_by_user": true});
                    if let Some(root) = target_root {
                        setting["target_root"] = root;
                    }
                    if let Some(name) = target_name {
                        setting["target_name"] = name;
                    }
                })?;
                // A commit's graph that is still being built writes into
                // the folder. It is told to end first, and removes the
                // folder when it has ended: it is told while it is known to
                // run, so it cannot end without having heard.
                let growing = lock(&self.revisions)?
                    .get(project)
                    .map(|(_, job)| {
                        job.purge.store(true, Ordering::Release);
                        job.cancel.store(true, Ordering::Release);
                    })
                    .is_some();
                let running = lock(&self.jobs)?.get(project).cloned();
                match running {
                    // The build owns the folder until it has ended.
                    Some(job) => {
                        job.purge.store(true, Ordering::Release);
                        job.cancel.store(true, Ordering::Release);
                    }
                    None => {
                        self.cancel_live_json_writer(project);
                        self.evict_ir_store(project);
                        let home = folder(&self.store.state, project);
                        // Under a commit's build the folder may not go at
                        // once: what that build still writes goes with it.
                        if home.exists()
                            && let Err(error) = remove_settling(&home)
                            && !growing
                        {
                            return Err(error.into());
                        }
                    }
                }
                self.clear_project_chosen(project)?;
                lock(&self.refused)?.retain(|(name, _), _| name != project);
                let plock = self.project_publish_lock(project);
                let _publish_guard = lock(&plock)?;
                let rev = self.bump_ir_revision(project);
                self.maps.forget(project);
                lock(&self.history)?.remove(project);
                self.emit_status_event(project, "off", "deleted", rev, None, None, None);
            }
            _ => {
                return Err(GraphError::invalid(
                    "Choose enable, disable, offer, withhold, rebuild or delete.",
                ));
            }
        }
        if matches!(
            action,
            "enable" | "disable" | "offer" | "withhold" | "rebuild"
        ) {
            let status_val = self.status(project, false)?;
            let state = status_val["state"].as_str().unwrap_or("off");
            let reason = match action {
                "enable" => "enabled",
                "disable" => "disabled",
                "offer" => "agents_offered",
                "withhold" => "agents_withheld",
                _ => "rebuild",
            };
            let rev = self.ir_revision_of(project);
            self.emit_status_event(
                project,
                state,
                reason,
                rev,
                status_val["graph"]["nodes"].as_u64(),
                status_val["graph"]["edges"].as_u64(),
                status_val["message"].as_str().map(str::to_owned),
            );
            return Ok(status_val);
        }
        self.status(project, false)
    }
    pub(super) fn shutdown_watchers(&self) {
        if let Ok(mut watchers) = self.watchers.lock() {
            let owned = std::mem::take(&mut *watchers);
            drop(watchers);
            for mut watcher in owned.into_values() {
                watcher.shutdown();
            }
        }
    }
    /// Ends every build; the tool and its workers do not outlive the Harness.
    pub fn close(&self) {
        self.stopping.store(true, Ordering::Release);
        self.shutdown_watchers();
        if let Ok(mut writers) = self.live_json_writers.lock() {
            let entries: Vec<_> = std::mem::take(&mut *writers).into_values().collect();
            drop(writers);
            for slot in entries {
                slot.cancel.store(true, Ordering::Release);
                let _guard = slot.done_lock.lock();
            }
        }
        if let Ok(mut stores) = self.ir_stores.lock() {
            stores.clear();
        }
        if let Ok(mut stores_ro) = self.ir_stores_ro.lock() {
            stores_ro.clear();
        }
        crate::graph_ir::GraphIrStore::evict_readonly_cache_for_dir(
            &self.store.state.join("graphs"),
        );
        let workers = match self.workers.lock() {
            Ok(mut workers) => std::mem::take(&mut *workers),
            Err(_) => return,
        };
        for worker in workers {
            let _ = worker.join();
        }
    }
}
