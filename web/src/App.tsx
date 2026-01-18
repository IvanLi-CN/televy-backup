import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useState } from "react";

type RpcSettings = {
	sources: string[];
	schedule: {
		enabled: boolean;
		kind: "hourly" | "daily" | string;
		hourlyMinute: number;
		dailyAt: string;
		timezone: string;
	};
	retention: { keepLastSnapshots: number };
	chunking: { minBytes: number; avgBytes: number; maxBytes: number };
	telegram: {
		mode: "botapi" | string;
		chatId: string;
		botTokenKey: string;
		rateLimit: { maxConcurrentUploads: number; minDelayMs: number };
	};
};

type SecretsStatus = {
	telegramBotTokenPresent: boolean;
	masterKeyPresent: boolean;
};

type TaskStateEvent = {
	taskId: string;
	kind: "backup" | "restore" | "verify" | string;
	state: "queued" | "running" | "succeeded" | "failed" | "cancelled" | string;
	error?: { code: string; message: string } | null;
};

type TaskProgressEvent = {
	taskId: string;
	phase: string;
	filesTotal?: number;
	filesDone?: number;
	chunksTotal?: number;
	chunksDone?: number;
	bytesRead?: number;
	bytesUploaded?: number;
	bytesDeduped?: number;
};

type TaskView = {
	taskId: string;
	kind?: string;
	state?: string;
	phase?: string;
	error?: { code: string; message: string } | null;
	progress?: TaskProgressEvent;
};

type Tab = "backup" | "restore" | "verify" | "tasks" | "settings";

export default function App() {
	const [tab, setTab] = useState<Tab>("backup");
	const [settings, setSettings] = useState<RpcSettings | null>(null);
	const [secrets, setSecrets] = useState<SecretsStatus | null>(null);
	const [tasks, setTasks] = useState<Record<string, TaskView>>({});
	const [snapshots, setSnapshots] = useState<string[]>([]);

	useEffect(() => {
		void invoke<{ settings: RpcSettings; secrets: SecretsStatus }>(
			"settings_get",
		).then((res) => {
			setSettings(res.settings);
			setSecrets(res.secrets);
		});

		const unsubs: Array<() => void> = [];
		void listen<TaskStateEvent>("task:state", (e) => {
			const p = e.payload;
			setTasks((prev) => ({
				...prev,
				[p.taskId]: {
					...(prev[p.taskId] ?? { taskId: p.taskId }),
					taskId: p.taskId,
					kind: p.kind,
					state: p.state,
					error: p.error ?? null,
				},
			}));
		}).then((unsub) => unsubs.push(unsub));

		void listen<TaskProgressEvent>("task:progress", (e) => {
			const p = e.payload;
			setTasks((prev) => ({
				...prev,
				[p.taskId]: {
					...(prev[p.taskId] ?? { taskId: p.taskId }),
					taskId: p.taskId,
					phase: p.phase,
					progress: p,
				},
			}));
		}).then((unsub) => unsubs.push(unsub));

		return () => {
			for (const u of unsubs) u();
		};
	}, []);

	const tasksList = useMemo(() => Object.values(tasks), [tasks]);

	const reloadSnapshots = async () => {
		const ids = await invoke<string[]>("backup_list_snapshots");
		setSnapshots(ids);
	};

	if (!settings) {
		return (
			<main className="container">
				<h1>TelevyBackup</h1>
				<p>Loading…</p>
			</main>
		);
	}

	return (
		<main className="container">
			<header style={{ display: "flex", gap: 8, alignItems: "center" }}>
				<h1 style={{ marginRight: "auto" }}>TelevyBackup</h1>
				<TabButton tab="backup" current={tab} onClick={setTab} />
				<TabButton tab="restore" current={tab} onClick={setTab} />
				<TabButton tab="verify" current={tab} onClick={setTab} />
				<TabButton tab="tasks" current={tab} onClick={setTab} />
				<TabButton tab="settings" current={tab} onClick={setTab} />
			</header>

			{tab === "settings" ? (
				<SettingsView
					settings={settings}
					secrets={secrets}
					onChange={setSettings}
					onSecretsChange={setSecrets}
				/>
			) : null}

			{tab === "backup" ? (
				<BackupView settings={settings} onTask={reloadSnapshots} />
			) : null}
			{tab === "restore" ? (
				<RestoreView
					snapshots={snapshots}
					onReloadSnapshots={reloadSnapshots}
				/>
			) : null}
			{tab === "verify" ? (
				<VerifyView snapshots={snapshots} onReloadSnapshots={reloadSnapshots} />
			) : null}
			{tab === "tasks" ? <TasksView tasks={tasksList} /> : null}
		</main>
	);
}

function TabButton(props: {
	tab: Tab;
	current: Tab;
	onClick: (t: Tab) => void;
}) {
	return (
		<button
			type="button"
			onClick={() => props.onClick(props.tab)}
			style={{
				padding: "6px 10px",
				borderRadius: 8,
				border: "1px solid #444",
				background: props.current === props.tab ? "#2c2c2c" : "#1c1c1c",
				color: "white",
			}}
		>
			{props.tab}
		</button>
	);
}

function SettingsView(props: {
	settings: RpcSettings;
	secrets: SecretsStatus | null;
	onChange: (s: RpcSettings) => void;
	onSecretsChange: (s: SecretsStatus) => void;
}) {
	const [botToken, setBotToken] = useState<string>("");
	const [status, setStatus] = useState<string>("");
	const [stats, setStats] = useState<string>("");

	const save = async () => {
		setStatus("Saving…");
		const settings = await invoke<RpcSettings>("settings_set", {
			req: {
				settings: props.settings,
				secrets: {
					telegramBotToken: botToken.length ? botToken : null,
					rotateMasterKey: false,
				},
			},
		});
		props.onChange(settings);
		const res = await invoke<{ settings: RpcSettings; secrets: SecretsStatus }>(
			"settings_get",
		);
		props.onSecretsChange(res.secrets);
		setBotToken("");
		setStatus("Saved.");
	};

	const validate = async () => {
		setStatus("Validating…");
		try {
			const res = await invoke<{ botUsername: string; chatId: string }>(
				"telegram_validate",
			);
			setStatus(`OK: @${res.botUsername}, chatId=${res.chatId}`);
		} catch (e) {
			setStatus(`Failed: ${String(e)}`);
		}
	};

	const loadStats = async () => {
		const res = await invoke<{
			snapshotsTotal: number;
			chunksTotal: number;
			bytesUploadedTotal: number;
			bytesDedupedTotal: number;
		}>("stats_get");
		setStats(
			`snapshots=${res.snapshotsTotal}, chunks=${res.chunksTotal}, bytes=${res.bytesUploadedTotal}`,
		);
	};

	return (
		<section style={{ marginTop: 16 }}>
			<h2>Settings</h2>
			<p style={{ opacity: 0.8 }}>
				Secrets: botToken=
				{props.secrets?.telegramBotTokenPresent ? "yes" : "no"}, masterKey=
				{props.secrets?.masterKeyPresent ? "yes" : "no"}
			</p>

			<div style={{ display: "grid", gap: 8, maxWidth: 800 }}>
				<label>
					<span>Telegram chatId</span>
					<input
						value={props.settings.telegram.chatId}
						onChange={(e) =>
							props.onChange({
								...props.settings,
								telegram: {
									...props.settings.telegram,
									chatId: e.target.value,
								},
							})
						}
					/>
				</label>
				<label>
					<span>Telegram bot token (stored in Keychain)</span>
					<input
						value={botToken}
						onChange={(e) => setBotToken(e.target.value)}
					/>
				</label>

				<label>
					<span>Sources (one per line)</span>
					<textarea
						rows={4}
						value={props.settings.sources.join("\n")}
						onChange={(e) =>
							props.onChange({
								...props.settings,
								sources: e.target.value
									.split("\n")
									.map((s) => s.trim())
									.filter(Boolean),
							})
						}
					/>
				</label>

				<label>
					<span>Retention keepLastSnapshots</span>
					<input
						type="number"
						value={props.settings.retention.keepLastSnapshots}
						onChange={(e) =>
							props.onChange({
								...props.settings,
								retention: { keepLastSnapshots: Number(e.target.value) },
							})
						}
					/>
				</label>

				<div style={{ display: "flex", gap: 8 }}>
					<button type="button" onClick={save}>
						Save
					</button>
					<button type="button" onClick={validate}>
						Validate Telegram
					</button>
					<button type="button" onClick={() => void loadStats()}>
						Load stats
					</button>
					<span style={{ opacity: 0.8 }}>{status}</span>
				</div>
				{stats.length ? <div style={{ opacity: 0.8 }}>{stats}</div> : null}
			</div>
		</section>
	);
}

function BackupView(props: { settings: RpcSettings; onTask: () => void }) {
	const [sourcePath, setSourcePath] = useState<string>(
		props.settings.sources[0] ?? "",
	);
	const [label, setLabel] = useState<string>("");
	const [status, setStatus] = useState<string>("");

	const start = async () => {
		setStatus("Starting…");
		try {
			const res = await invoke<{ taskId: string; snapshotId: string }>(
				"backup_start",
				{
					req: { sourcePath, label },
				},
			);
			setStatus(`Started taskId=${res.taskId} snapshotId=${res.snapshotId}`);
			await props.onTask();
		} catch (e) {
			setStatus(`Failed: ${String(e)}`);
		}
	};

	return (
		<section style={{ marginTop: 16 }}>
			<h2>Backup</h2>
			<div style={{ display: "grid", gap: 8, maxWidth: 800 }}>
				<label>
					<span>Source path</span>
					<input
						value={sourcePath}
						onChange={(e) => setSourcePath(e.target.value)}
					/>
				</label>
				<label>
					<span>Label</span>
					<input value={label} onChange={(e) => setLabel(e.target.value)} />
				</label>
				<div style={{ display: "flex", gap: 8 }}>
					<button type="button" onClick={start}>
						Start backup
					</button>
					<span style={{ opacity: 0.8 }}>{status}</span>
				</div>
			</div>
		</section>
	);
}

function RestoreView(props: {
	snapshots: string[];
	onReloadSnapshots: () => Promise<void>;
}) {
	const [snapshotId, setSnapshotId] = useState<string>("");
	const [targetPath, setTargetPath] = useState<string>("");
	const [status, setStatus] = useState<string>("");

	return (
		<section style={{ marginTop: 16 }}>
			<h2>Restore</h2>
			<div style={{ display: "grid", gap: 8, maxWidth: 900 }}>
				<div style={{ display: "flex", gap: 8 }}>
					<button type="button" onClick={() => void props.onReloadSnapshots()}>
						Reload snapshots
					</button>
					<select
						value={snapshotId}
						onChange={(e) => setSnapshotId(e.target.value)}
					>
						<option value="">Select snapshot…</option>
						{props.snapshots.map((id) => (
							<option key={id} value={id}>
								{id}
							</option>
						))}
					</select>
				</div>

				<label>
					<span>Target path (must be empty dir)</span>
					<input
						value={targetPath}
						onChange={(e) => setTargetPath(e.target.value)}
					/>
				</label>
				<div style={{ display: "flex", gap: 8 }}>
					<button
						type="button"
						onClick={() => {
							setStatus("Starting…");
							void invoke<{ taskId: string }>("restore_start", {
								req: { snapshotId, targetPath },
							})
								.then((res) => setStatus(`Started taskId=${res.taskId}`))
								.catch((e) => setStatus(`Failed: ${String(e)}`));
						}}
					>
						Start restore
					</button>
					<span style={{ opacity: 0.8 }}>{status}</span>
				</div>
			</div>
		</section>
	);
}

function VerifyView(props: {
	snapshots: string[];
	onReloadSnapshots: () => Promise<void>;
}) {
	const [snapshotId, setSnapshotId] = useState<string>("");
	const [status, setStatus] = useState<string>("");

	return (
		<section style={{ marginTop: 16 }}>
			<h2>Verify</h2>
			<div style={{ display: "grid", gap: 8, maxWidth: 900 }}>
				<div style={{ display: "flex", gap: 8 }}>
					<button type="button" onClick={() => void props.onReloadSnapshots()}>
						Reload snapshots
					</button>
					<select
						value={snapshotId}
						onChange={(e) => setSnapshotId(e.target.value)}
					>
						<option value="">Select snapshot…</option>
						{props.snapshots.map((id) => (
							<option key={id} value={id}>
								{id}
							</option>
						))}
					</select>
				</div>
				<div style={{ display: "flex", gap: 8 }}>
					<button
						type="button"
						onClick={() => {
							setStatus("Starting…");
							void invoke<{ taskId: string }>("verify_start", {
								req: { snapshotId },
							})
								.then((res) => setStatus(`Started taskId=${res.taskId}`))
								.catch((e) => setStatus(`Failed: ${String(e)}`));
						}}
					>
						Start verify
					</button>
					<span style={{ opacity: 0.8 }}>{status}</span>
				</div>
			</div>
		</section>
	);
}

function TasksView(props: { tasks: TaskView[] }) {
	return (
		<section style={{ marginTop: 16 }}>
			<h2>Tasks</h2>
			{props.tasks.length === 0 ? <p>No tasks yet.</p> : null}
			<div style={{ display: "grid", gap: 8 }}>
				{props.tasks.map((t) => (
					<div
						key={t.taskId}
						style={{
							border: "1px solid #333",
							borderRadius: 8,
							padding: 10,
							background: "#121212",
						}}
					>
						<div style={{ display: "flex", gap: 8, alignItems: "center" }}>
							<strong>{t.kind ?? "task"}</strong>
							<span style={{ opacity: 0.8 }}>id={t.taskId}</span>
							<span style={{ marginLeft: "auto" }}>{t.state ?? "unknown"}</span>
						</div>
						<div style={{ display: "flex", gap: 8, marginTop: 6 }}>
							{(t.state === "running" || t.state === "queued") &&
							t.kind !== "verify" ? (
								<button
									type="button"
									onClick={() => {
										const cmd =
											t.kind === "restore" ? "restore_cancel" : "backup_cancel";
										void invoke<boolean>(cmd, { task_id: t.taskId });
									}}
								>
									Cancel
								</button>
							) : null}
						</div>
						<div style={{ opacity: 0.85 }}>
							phase={t.phase ?? t.progress?.phase ?? "?"}
							{t.progress?.bytesUploaded != null
								? `, uploaded=${t.progress.bytesUploaded}`
								: ""}
							{t.progress?.bytesDeduped != null
								? `, deduped=${t.progress.bytesDeduped}`
								: ""}
						</div>
						{t.error ? (
							<div style={{ color: "#ff6b6b" }}>
								{t.error.code}: {t.error.message}
							</div>
						) : null}
					</div>
				))}
			</div>
		</section>
	);
}
