use super::*;

impl App {
    pub(super) fn watch(&self, runtime: &Runtime) -> i32 {
        let mut phases = Phases::start();
        self.hook_console();
        let state_root = self.paths.state_root();
        let settings = Arc::new(Store::new(state_root.join("settings.json"), {
            let ux = self.ux.clone();
            Box::new(move |msg: &str| ux.warn(Component::Terminal, msg))
        }));
        let prefs = settings.snapshot();
        self.banner(&state_root);
        if self.build.dev {
            let json = serde_json::to_string(&prefs).unwrap_or_default();
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
        let og_hint = Arc::new(OgFallbackHint::new(og_hint::DEFAULT_TTL, clock::system()));
        let health = Arc::new(HealthGate::new(
            health::DEFAULT_COOLDOWN,
            health::DEFAULT_VERDICT_TIMEOUT,
            clock::system(),
        ));
        let codecs = Arc::new(CodecCapabilities::system(self.log.clone()));

        let ipc = Ipc::builder(Arc::new(mesh.clone()), self.ux.clone(), self.log.clone())
            .cache(std::sync::Arc::clone(&cache))
            .og_hint(std::sync::Arc::clone(&og_hint))
            .health(std::sync::Arc::clone(&health))
            .stats(std::sync::Arc::clone(&stats))
            .high_quality({
                let settings = std::sync::Arc::clone(&settings);
                move || settings.snapshot().playback.high_quality
            })
            .verified_codecs({
                let codecs = std::sync::Arc::clone(&codecs);
                move || codecs.verified()
            })
            .report_fallback({
                let reporter = std::sync::Arc::clone(&self.reporter);
                move |req, reason| reporter.report_fallback(req, reason, None)
            })
            .build();
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
        let relay = if ports.initialize() {
            let https_allowed = prefs.https_enabled();
            if !https_allowed {
                self.ux.write(
                    Component::Relay,
                    "secure local video disabled in settings; using local HTTP fallback.",
                );
            }
            let relay_started = runtime.block_on(self.start_relay(
                &ports,
                https_allowed,
                &state_root,
                mesh.first_party(),
                std::sync::Arc::clone(&activity),
                std::sync::Arc::clone(&stats),
            ));
            if relay_started.is_none() {
                self.ux.warn(
                    Component::Relay,
                    "local video relay could not start -- public-instance local video disabled.",
                );
            }
            relay_started
        } else {
            self.ux.warn(
                Component::Relay,
                "could not reserve a local video port -- public-instance local video disabled.",
            );
            None
        };

        phases.mark("relay");

        let mut deps = MonitorDeps::new(
            Arc::new(mesh.clone()),
            mesh.first_party(),
            self.ux.clone(),
            self.log.clone(),
        );
        deps.cache = Some(std::sync::Arc::clone(&cache));
        deps.og_hint = Some(og_hint);
        deps.health = Some(health);
        if let Some((server, scheme)) = relay {
            let bypass = Arc::new(RelayBypass::new(
                std::sync::Arc::clone(&ports),
                scheme,
                self.ux.clone(),
            ));
            let engage = std::sync::Arc::clone(&bypass);
            deps.relay_wedge = Some(RelayWedge {
                activity,
                on_wedged: Box::new(move |port| engage.engage(port)),
            });
            deps.on_new_session = Some(Box::new(move || bypass.release()));
            put(&self.parts.relay, Arc::new(server));
        }
        let vrc_dir = self.paths.local_low().join("VRChat").join("VRChat");
        let cancel = CancellationToken::new();
        let handle = VrcLogMonitor::new(deps, vrc_dir).spawn(clock::system(), cancel.clone());
        put(&self.parts.logmon, (cancel, handle));
        phases.mark("logmon");

        if !patcher.start() {
            self.life.shutdown("patch-refused", false, None);
            return 2;
        }
        phases.mark("patch");

        self.ux.success(
            Component::Patch,
            "VRChat video hook ready; watching for game updates.",
        );
        self.ux.write(
            Component::Terminal,
            "type /help for commands, /status for activity, /settings for options.",
        );
        self.ux.write(
            Component::Terminal,
            "to uninstall, run vrcresolver.Uninstaller.exe from this folder.",
        );

        self.start_hosts();
        phases.mark("hosts");

        let cancel = CancellationToken::new();
        let beat = Arc::new(Heartbeat {
            stats: std::sync::Arc::clone(&stats),
            mesh_connected: Box::new({
                let mesh = mesh.clone();
                move || mesh.is_connected()
            }),
            cache_size: Box::new({
                let cache = cache;
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

        let _ = update::start_background_check(
            crate::maintenance::update::CheckPolicy {
                enabled: prefs.maintenance.update_check,
                include_prereleases: prefs.maintenance.include_prereleases,
            },
            &self.build.version,
            self.ux.clone(),
            &self.log,
        );
        codecs.refresh();
        let _ = codec::start_background_check(
            prefs.maintenance.codec_auto_install,
            &state_root,
            codecs,
            self.ux.clone(),
            self.log.clone(),
        );
        tools::sweep_legacy_install_tools(&self.install_dir);
        if let Err(e) = stage_wrapper_hashes(&self.install_dir, &state_root) {
            self.log.line(&format!(
                "[startup] could not stage wrapper_hashes for wrapper: {e}"
            ));
        }

        phases.mark("background");

        self.start_terminal(settings, stats, mesh, &state_root, started);
        phases.mark("terminal");
        if self.build.dev {
            self.log.line(&phases.line());
        }

        self.life.wait_quit();
        self.ux.write(
            Component::Shutdown,
            "shutting down; restoring VRChat tools and closing local services.",
        );
        self.life.shutdown("quit", false, None);
        0
    }

    pub(super) async fn start_relay(
        &self,
        ports: &RelayPortManager,
        https_allowed: bool,
        state_root: &Path,
        policy: Arc<std::sync::RwLock<vrcr_shared::first_party::FirstPartyPolicy>>,
        activity: Arc<RelayActivity>,
        stats: Arc<Stats>,
    ) -> Option<(RelayServer, &'static str)> {
        let upstream: Result<Arc<dyn Upstream>, String> = connect::platform_tls()
            .and_then(|tls| ReqwestUpstream::new((*tls).clone()))
            .map(upstream::shared);
        let tls_host = WindowsTlsHost::current().ok();
        let tls_for = |port: u16| -> ServerTls {
            let host = tls_host.as_ref()?;
            RelayTls::new(host, state_root, self.ux.clone(), clock::system()).ensure_ready(port)
        };
        let hitch = Arc::new(HitchDetector::new());
        let start = |port: u16, tls: ServerTls| {
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
            let ux = self.ux.clone();
            let log = self.log.clone();
            async move {
                let upstream = upstream.map_err(io::Error::other)?;
                let deps = RelayDeps {
                    upstream,
                    policy,
                    activity,
                    hitch,
                    stats,
                    now: clock::system(),
                    ux,
                    log,
                };
                RelayServer::start(cfg, deps).await
            }
        };
        start_relay(ports, https_allowed, tls_for, start, &self.ux).await
    }
}
