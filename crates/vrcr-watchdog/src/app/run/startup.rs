use super::*;
use vrcr_shared::trust_gateway;

pub(super) struct RelayShared {
    recovery: crate::relay::retry::Hooks,
    activity: Arc<RelayActivity>,
    stats: Arc<Stats>,
    manifests: Arc<Manifests>,
}

impl App {
    pub(super) fn watch(&self, runtime: &Runtime) -> i32 {
        let mut phases = Phases::start();
        let state_root = self.paths.state_root();
        let previous = crate::diagnostics::startup::begin(&state_root);
        self.hook_console();
        let settings = Arc::new(Store::new(state_root.join("settings.json"), {
            let ux = self.ux.clone();
            Box::new(move |msg: &str| ux.log_warn(Component::Terminal, msg))
        }));
        let include_prereleases = settings.include_prereleases();
        self.banner(&state_root);
        if self.build.dev {
            let json = serde_json::json!({ "maintenance": { "include_prereleases": include_prereleases } }).to_string();
            self.log
                .line(&perf::system_line(&SystemInfo::probe(&state_root, json)));
        }
        phases.mark("settings");

        patch::log_vrc_process_state(&self.ux);
        let patcher = Arc::new(PatchManager::new(
            PatchConfig {
                install_dir: self.install_dir.clone(),
                vrc_tools_dir: self.paths.vrc_tools_dir(None),
                state_dir: state_root.clone(),
                tick_delay: patch::DEFAULT_TICK_DELAY,
            },
            self.ux.clone(),
            self.log.clone(),
        ));
        put(&self.parts.patch, std::sync::Arc::clone(&patcher));
        self.install_shutdown(std::sync::Arc::clone(&patcher));
        session_end::listen(&self.life, &self.ux);
        patcher.recover_from_unclean_shutdown();
        phases.mark("recover");

        let started = SystemTime::now();
        let stats = Arc::new(Stats::new(started));
        if self.build.dev {
            stats.perf.enable();
        }
        let mesh = MeshClient::new(
            Config::new(&state_root, &self.build.version),
            self.ux.clone(),
            self.log.clone(),
            std::sync::Arc::clone(&stats),
        );
        put(&self.parts.mesh, mesh.clone());
        let cache = Arc::new(ResolveCache::new(
            state_root.join("resolve_cache.json"),
            clock::system(),
            self.log.clone(),
        ));
        put(&self.parts.cache, std::sync::Arc::clone(&cache));
        let og_hint = Arc::new(OgFallbackHint::new(clock::system()));
        let emitted = Arc::new(EmittedUrls::new(clock::system()));
        let health = Arc::new(HealthGate::new(
            health::DEFAULT_COOLDOWN,
            health::DEFAULT_VERDICT_TIMEOUT,
            clock::system(),
        ));
        let codecs = Arc::new(CodecCapabilities::system(self.log.clone()));

        let manifests = Arc::new(Manifests::default());
        let is_relay_up = Arc::new(AtomicBool::new(false));
        let local_step = self.local_step(
            &state_root,
            std::sync::Arc::clone(&manifests),
            std::sync::Arc::clone(&is_relay_up),
        );

        let ipc = Ipc::builder(Arc::new(mesh.clone()), self.ux.clone(), self.log.clone())
            .cache(std::sync::Arc::clone(&cache))
            .og_hint(std::sync::Arc::clone(&og_hint))
            .health(std::sync::Arc::clone(&health))
            .stats(std::sync::Arc::clone(&stats))
            .verified_codecs({
                let codecs = std::sync::Arc::clone(&codecs);
                move || codecs.verified()
            })
            .report_fallback({
                let reporter = std::sync::Arc::clone(&self.reporter);
                move |req, reason| reporter.report_fallback(req, reason, None)
            })
            .local(local_step)
            .emitted(std::sync::Arc::clone(&emitted))
            .first_party(mesh.first_party())
            .native_fallback(mesh.native_fallback())
            .build();
        mesh.on_revocation({
            let emitted = std::sync::Arc::clone(&emitted);
            let cache = std::sync::Arc::clone(&cache);
            let og_hint = std::sync::Arc::clone(&og_hint);
            let ux = self.ux.clone();
            move |revocation| {
                let Some(origin) = emitted.origin_for_revocation(
                    &revocation.request_id,
                    revocation.correlation_id.as_deref(),
                    revocation.url.as_deref(),
                ) else {
                    return;
                };
                let (evicted, lease) =
                    og_hint::arm(Some(&cache), Some(&og_hint), &origin.source_url);
                ux.log_write(
                    Component::Mesh,
                    &format!(
                        "server withdrew a resolved link host={} reason={} evicted={evicted} og_hint={}",
                        vrcr_shared::logutil::bare_host(&origin.source_url),
                        vrcr_shared::logutil::sanitize_for_console(
                            revocation.reason.as_deref().unwrap_or("?"),
                            32
                        ),
                        og_hint::lease_label(lease)
                    ),
                );
            }
        });
        put(
            &self.parts.ipc,
            vec![ipc.serve(wire::PIPE_NAME), ipc.serve(legacy::PIPE_NAME)],
        );
        mesh.start();
        phases.mark("ipc");

        self.raise_wininet(&state_root);
        phases.mark("wininet");

        let ports = Arc::new(RelayPortManager::new(&state_root, self.ux.clone()));
        put(&self.parts.ports, std::sync::Arc::clone(&ports));
        let activity = Arc::new(RelayActivity::new());
        let port_ready = ports.initialize();
        let hosts = Arc::new(HostsManager::system(
            self.ux.clone(),
            Box::new({
                let ux = self.ux.clone();
                let closing = self.life.closing.clone();
                move || setup::approve(permission::ask(setup::Changes::Hosts, &closing, &ux), &ux)
            }),
        ));
        let setup_state = Arc::new(repair::SetupState::starting(self.life.closing.clone()));
        let bypass = Arc::new(OnceLock::<RelayBypass>::new());

        let mut deps = MonitorDeps::new(
            Arc::new(mesh.clone()),
            mesh.first_party(),
            self.ux.clone(),
            self.log.clone(),
        );
        deps.cache = Some(Arc::clone(&cache));
        deps.stats = Some(Arc::clone(&stats));
        deps.og_hint = Some(std::sync::Arc::clone(&og_hint));
        deps.emitted = Some(std::sync::Arc::clone(&emitted));
        deps.health = Some(health);
        deps.local = Some(Arc::clone(&manifests));
        deps.relay_wedge = Some(RelayWedge {
            activity: Arc::clone(&activity),
            on_wedged: Box::new({
                let bypass = Arc::clone(&bypass);
                move |port| bypass.get().is_some_and(|bypass| bypass.engage(port))
            }),
        });
        deps.on_new_session = Some(Box::new({
            let bypass = Arc::clone(&bypass);
            move || {
                if let Some(bypass) = bypass.get() {
                    bypass.release();
                }
            }
        }));
        if !patcher.start() {
            self.life.shutdown("patch-refused", false, None);
            return 2;
        }
        match vrchat::spawn(
            Box::new(VrchatProcesses),
            Arc::new(patch::PatchSession::new(
                Arc::clone(&patcher),
                &state_root,
                self.ux.clone(),
            )),
            vrchat::Timing::LIVE,
        ) {
            Some(watch) => put(&self.parts.vrchat, watch),
            None => self.ux.log_warn(
                Component::Patch,
                "could not start the VRChat exit watcher; yt-dlp.exe stays patched until VRCResolver closes",
            ),
        }
        phases.mark("patch");

        self.ux.event(vrcr_shared::console::Notice::Ready);
        self.ux.log_success(
            Component::Patch,
            "VRChat video hook ready; watching for game updates.",
        );
        self.ux
            .log_write(Component::Terminal, "Type help for commands.");
        self.ux.log_write(
            Component::Terminal,
            "to uninstall, run vrcresolver.Uninstaller.exe from this folder.",
        );

        deps.setup = Some(SetupProbe {
            pass: Box::new({
                let setup_state = Arc::clone(&setup_state);
                move || setup_state.pass()
            }),
            incomplete: Box::new({
                let hosts = Arc::clone(&hosts);
                let ux = self.ux.clone();
                let state_root = state_root.clone();
                move || {
                    let hosts_missing = hosts.read_state().ok().map(|present| !present);
                    if hosts_missing == Some(true) || !port_ready {
                        return hosts_missing;
                    }
                    let host = WindowsTlsHost::current().ok()?;
                    match RelayTls::new(&host, &state_root, ux.clone(), clock::system())
                        .trust_missing()
                        .ok()?
                    {
                        true => Some(true),
                        false => hosts_missing,
                    }
                }
            }),
        });
        deps.wrapper_missing = Some(Box::new({
            let patcher = Arc::clone(&patcher);
            move || patcher.wrapper_missing()
        }));
        let vrc_dir = self.paths.local_low().join("VRChat").join("VRChat");
        let cancel = CancellationToken::new();
        let handle = VrcLogMonitor::new(deps, vrc_dir).spawn(clock::system(), cancel.clone());
        put(&self.parts.logmon, (cancel, handle));
        phases.mark("logmon");

        let cancel = CancellationToken::new();
        let beat = Arc::new(Heartbeat {
            stats: std::sync::Arc::clone(&stats),
            mesh_connected: Box::new({
                let mesh = mesh.clone();
                move || mesh.is_connected()
            }),
            cache_size: Box::new({
                let cache = Arc::clone(&cache);
                move || cache.count()
            }),
            now: clock::system(),
            ux: self.ux.clone(),
            log: self.log.clone(),
        });
        put(
            &self.parts.heartbeat,
            (cancel.clone(), beat.spawn(heartbeat::INTERVAL, cancel)),
        );
        if self.build.dev {
            let cancel = CancellationToken::new();
            let reporter = Arc::new(PerfReporter {
                stats: std::sync::Arc::clone(&stats),
                cores: perf::logical_cores(),
                now: clock::system(),
                log: self.log.clone(),
            });
            put(
                &self.parts.perf,
                (cancel.clone(), reporter.spawn(perf::INTERVAL, cancel)),
            );
        }

        let update_gate = update::UpdateGate::default();
        let prompter = self.update_prompter(&settings, &update_gate);
        let manual_prompter = prompter.clone();
        let update_runtime = runtime.handle().clone();
        let update_check: UpdateCheck =
            Arc::new(move || update::start_manual_check(&manual_prompter, &update_runtime));
        drop(update::start_background_check(
            include_prereleases,
            prompter,
        ));
        codecs.refresh();
        drop(codec::start_background_check(
            &state_root,
            codecs,
            self.ux.clone(),
            self.log.clone(),
        ));
        tools::sweep_legacy_install_tools(&self.install_dir);
        if let Err(e) = stage_wrapper_hashes(&self.install_dir, &state_root) {
            self.log.line(&format!(
                "[startup] could not stage wrapper_hashes for wrapper: {e}"
            ));
        }

        phases.mark("background");

        let repair = repair::command(
            self.ux.clone(),
            Arc::clone(&hosts),
            Arc::clone(&ports),
            &state_root,
            Arc::clone(&setup_state),
            patcher,
        );
        self.start_terminal(
            settings,
            Arc::clone(&stats),
            mesh.clone(),
            &state_root,
            repair,
            update_check,
        );
        phases.mark("terminal");

        let permission_worker = runtime.spawn_blocking({
            let hosts = Arc::clone(&hosts);
            let ux = self.ux.clone();
            let state_root = state_root.clone();
            let closing = self.life.closing.clone();
            let port = ports.current_port();
            move || {
                let (tls, hosts) = setup::run(
                    &hosts,
                    false,
                    &closing,
                    &|changes| permission::ask(changes, &closing, &ux),
                    &ux,
                    |approve| {
                        let host = WindowsTlsHost::current().map_err(|error| {
                            ux.event(vrcr_shared::console::Notice::SecureSetupIncomplete);
                            ux.log_warn(Component::Relay, &error.to_string());
                            crate::maintenance::SetupError::Failed
                        })?;
                        RelayTls::new(&host, &state_root, ux.clone(), clock::system())
                            .with_permission(approve)
                            .setup_ready(port)
                    },
                );
                let reported = tls.is_err() || hosts.is_err();
                (tls.ok().flatten(), reported)
            }
        });
        let (relay, reported) = runtime.block_on(async {
            let (tls, reported) = tokio::select! {
                biased;
                () = self.life.closing.cancelled() => return (None, false),
                result = permission_worker => match result {
                    Ok(result) => result,
                    Err(error) => {
                        self.ux.event(vrcr_shared::console::Notice::SecureSetupIncomplete);
                        self.ux.log_warn(
                            Component::Terminal,
                            &format!("Video setup stopped: {}", panic_text(&error)),
                        );
                        (None, true)
                    }
                }
            };
            if !port_ready {
                self.ux
                    .event(vrcr_shared::console::Notice::LocalPortUnavailable);
                self.ux.log_warn(
                    Component::Relay,
                    "could not reserve a local video port -- public-instance local video disabled.",
                );
                return (None, reported);
            }
            let relay = self
                .start_relay(
                    &ports,
                    tls,
                    mesh.first_party(),
                    RelayShared {
                        recovery: crate::relay::retry::Hooks {
                            failed: Some({
                                let cache = Arc::clone(&cache);
                                let ux = self.ux.clone();
                                Arc::new(move |failure| {
                                    let source = failure
                                        .source_url
                                        .clone()
                                        .or_else(|| {
                                            emitted
                                                .origin_by_url(&failure.target_url)
                                                .map(|o| o.source_url)
                                        })
                                        .unwrap_or_else(|| failure.target_url.clone());
                                    let (evicted, lease) =
                                        og_hint::arm(Some(&cache), Some(&og_hint), &source);
                                    cache.evict_by_url(&failure.target_url);
                                    ux.log_write(
                                        Component::Relay,
                                        &format!(
                                            "relay route failed host={} reason={} evicted={evicted} og_hint={}",
                                            vrcr_shared::logutil::bare_host(&source),
                                            failure.reason,
                                            og_hint::lease_label(lease)
                                        ),
                                    );
                                })
                            }),
                            invalidate: Some(Arc::new(move |url| {
                                cache.evict_by_url(url);
                            })),
                            report: Some({
                                let mesh = mesh.clone();
                                Arc::new(move |feedback| mesh.send_playback_feedback(feedback))
                            }),
                        },
                        activity,
                        stats,
                        manifests,
                    },
                )
                .await;
            (relay, reported)
        });
        if self.life.closing.is_cancelled() {
            ports.delete_port_file();
        } else {
            self.log_relay_start(&mesh, relay.as_ref(), &hosts, &previous);
            if let Some((server, scheme)) = relay {
                let _ = bypass.set(RelayBypass::new(
                    Arc::clone(&ports),
                    scheme,
                    self.ux.clone(),
                ));
                setup_state
                    .relay_https
                    .store(scheme == "https", Ordering::Release);
                put(&self.parts.relay, Arc::new(server));
                is_relay_up.store(true, Ordering::SeqCst);
            } else if port_ready {
                self.ux
                    .event(vrcr_shared::console::Notice::RelayStartFailed);
                self.ux.log_warn(
                    Component::Relay,
                    "local video relay could not start -- public-instance local video disabled.",
                );
            }
            self.start_hosts_ticker(hosts);
            setup_state.finish(reported);
        }
        phases.mark("relay");
        if self.build.dev {
            self.log.line(&phases.line());
        }

        self.life.wait_for_close();
        self.ux.log_write(
            Component::Shutdown,
            "shutting down; restoring VRChat tools and closing local services.",
        );
        self.life.shutdown("console-exit", false, None);
        0
    }

    fn log_relay_start(
        &self,
        mesh: &MeshClient,
        relay: Option<&(RelayServer, &str)>,
        hosts: &HostsManager,
        previous: &str,
    ) {
        let node = mesh.current_node_host();
        let node = if node.is_empty() {
            crate::mesh::PROXY_HOST
        } else {
            &node
        };
        let hosts = match hosts.read_state() {
            Ok(true) => "present",
            Ok(false) => "missing",
            Err(_) => "unknown",
        };
        self.ux.log_write(
            Component::Relay,
            &crate::diagnostics::startup::line(&crate::diagnostics::startup::Environment {
                version: &self.build.version,
                os: &win::os_description(),
                server: node,
                port: relay.map_or(0, |(server, _)| server.port()),
                tls: relay.map_or("unavailable", |(_, scheme)| {
                    if *scheme == "https" {
                        "ready"
                    } else {
                        "not_in_use"
                    }
                }),
                hosts,
                vrchat: win::list_processes()
                    .iter()
                    .any(|process| win::process_stem(&process.exe).eq_ignore_ascii_case("VRChat")),
                previous,
            }),
        );
    }

    fn local_step(
        &self,
        state_root: &Path,
        manifests: Arc<Manifests>,
        is_relay_up: Arc<AtomicBool>,
    ) -> Option<Arc<dyn Step>> {
        let client = connect::platform_tls().and_then(|tls| local::client((*tls).clone()));
        let client = match client {
            Ok(client) => client,
            Err(e) => {
                self.log.line(&format!("[local] disabled: {e}"));
                return None;
            }
        };
        let root = state_root.to_path_buf();
        let relay = Arc::new(move || {
            if !is_relay_up.load(Ordering::SeqCst) {
                return None;
            }
            let port = trust_gateway::read_relay_port(&root)?;
            let scheme = trust_gateway::read_relay_scheme(&root);
            Some(format!("{scheme}://{}:{port}", trust_gateway::GATEWAY_HOST))
        });
        Some(Arc::new(Local::new(
            client,
            Origin::youtube(),
            manifests,
            relay,
        )))
    }

    pub(super) async fn start_relay(
        &self,
        ports: &RelayPortManager,
        tls: ServerTls,
        policy: Arc<std::sync::RwLock<vrcr_shared::first_party::FirstPartyPolicy>>,
        shared: RelayShared,
    ) -> Option<(RelayServer, &'static str)> {
        let RelayShared {
            recovery,
            activity,
            stats,
            manifests,
        } = shared;
        let upstream: Result<Arc<dyn Upstream>, String> = connect::platform_tls()
            .and_then(|tls| ReqwestUpstream::new((*tls).clone()))
            .map(upstream::shared);
        let hitch = Arc::new(HitchDetector::new());
        let start = |port: u16, tls: ServerTls| {
            let recovery = recovery.clone();
            let upstream = upstream.clone();
            let scheme = if tls.is_some() { "https" } else { "http" };
            let cfg = RelayConfig {
                port,
                scheme: scheme.to_owned(),
                timeout: None,
                tls,
                verbose: self.build.dev,
            };
            let policy = std::sync::Arc::clone(&policy);
            let activity = std::sync::Arc::clone(&activity);
            let hitch = std::sync::Arc::clone(&hitch);
            let stats = std::sync::Arc::clone(&stats);
            let manifests = std::sync::Arc::clone(&manifests);
            let ux = self.ux.clone();
            let log = self.log.clone();
            async move {
                let upstream = upstream.map_err(io::Error::other)?;
                let deps = RelayDeps {
                    recovery,
                    upstream,
                    policy,
                    activity,
                    hitch,
                    stats,
                    manifests,
                    now: clock::system(),
                    ux,
                    log,
                };
                RelayServer::start(cfg, deps).await
            }
        };
        start_relay(ports, tls.is_some(), |_| tls.clone(), start, &self.ux).await
    }
}
