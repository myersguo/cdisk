import { useEffect, useMemo, useRef, useState } from "react";
import * as api from "./api";
import { age, applySettings, bytes, categoryLabel, diskHealth, emptyTask, formatCount, isScan, mergeProgress, parsePaths, sections, selection, statusLabel, taskActive, visibleItems } from "./domain";
import type { Bootstrap, Candidate, HistoryEntry, Preview, Progress, Report, ScanMode, ScanTask, ValidationProgress, View } from "./domain";
import { LOCALE_OPTIONS, t, useLocale } from "./i18n";
import { localizeBackendText, localeTag } from "./locales";

function Icon({ name }: { name: string }) {
  const paths: Record<string, React.ReactNode> = {
    clean: <><path d="m15 3-5 8 4 3 6-8M9 10l6 4-4 7H3l3-8Z" /><path d="m6 21 3-5" /></>,
    code: <><path d="m8 6-6 6 6 6m8-12 6 6-6 6M14 4l-4 16" /></>,
    box: <><path d="m3 7 9-4 9 4v11l-9 4-9-4ZM3 7l9 5 9-5M12 12v10M7 5l10 5" /></>,
    disk: <><rect x="3" y="4" width="18" height="16" rx="3" /><path d="M3 14h18M7 17h2m3 0h1" /></>,
    history: <><path d="M3 10a9 9 0 1 1 2 8M3 4v6h6M12 7v6l4 2" /></>,
    shield: <path d="m12 2 8 4v6c0 5-8 10-8 10S4 17 4 12V6Zm-4 10 3 3 5-6" />,
    search: <><circle cx="10" cy="10" r="6" /><path d="m15 15 6 6" /></>,
    folder: <path d="M3 7V4h6l2 3h10v13H3Z" />,
    arrow: <path d="m9 5 7 7-7 7" />,
    refresh: <><path d="M20 10a8 8 0 0 0-14-5L3 8m0-5v5h5M4 14a8 8 0 0 0 14 5l3-3m0 5v-5h-5" /></>,
    check: <path d="m5 12 4 4L19 6" />,
    close: <path d="m6 6 12 12M6 18 18 6" />,
    file: <path d="M5 2h9l5 5v15H5Zm9 0v6h5M8 13h8m-8 4h8" />,
    globe: <><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" /></>,
  };
  return <svg viewBox="0 0 24 24" aria-hidden="true">{paths[name] ?? paths.disk}</svg>;
}

function Confirm({ preview, busy, onClose, onConfirm }: {
  preview: Preview; busy: boolean; onClose: () => void; onConfirm: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [ack, setAck] = useState(false);
  useEffect(() => { dialog.current?.showModal(); return () => dialog.current?.close(); }, []);
  return <dialog ref={dialog} className="confirm" onCancel={event => {
    event.preventDefault(); if (!busy) onClose();
  }}>
    <header><Icon name="shield" /><button aria-label={t("confirm.close")} disabled={busy} onClick={onClose}><Icon name="close" /></button></header>
    <h2>{t("confirm.title")}</h2>
    <p>{t("confirm.summary", { count: preview.items.length, size: bytes(preview.estimatedBytes) })}</p>
    <div className="confirm-items">{preview.items.map(item => <div key={item.id}>
      <strong>{localizeBackendText(item.title)}<span>{bytes(item.bytes)}</span></strong><code>{item.path}</code><small>{localizeBackendText(item.reason)}</small>
    </div>)}</div>
    <label className="ack"><input type="checkbox" checked={ack} onChange={e => setAck(e.target.checked)} disabled={busy} />
      {t("confirm.ack")}</label>
    <footer><button disabled={busy} onClick={onClose}>{t("confirm.back")}</button>
      <button className="danger-button" disabled={busy || !ack || !api.native} onClick={onConfirm}>
        {busy ? t("confirm.cleaning") : api.native ? t("confirm.remove") : t("confirm.demo")}
      </button></footer>
  </dialog>;
}

function History({ entries }: { entries: HistoryEntry[] }) {
  return <section className="page-scroll history-page">
    {!entries.length && <Empty icon="history" title={t("history.empty")} text={t("history.emptyText")} />}
    {entries.map(entry => <article className="history-entry" key={entry.id}>
      <header><div><h3>{entry.status === "complete" ? t("history.complete") : entry.status === "running" ? t("history.running") : t("history.partial")}</h3>
        <time>{new Date(entry.startedAt * 1000).toLocaleString(localeTag())}</time></div>
        <strong>{bytes(entry.estimatedBytes)}<small>{t("history.cleanedSize")}</small></strong></header>
      <p>{t("history.diskAvailable", { before: bytes(entry.availableBefore), after: entry.availableAfter === null ? t("history.unmeasured") : bytes(entry.availableAfter) })}
        <span>{t("history.spaceCaveat")}</span></p>
      {entry.items.map((item, i) => <details key={`${entry.id}-${i}`}><summary>
        <span className={item.status === "removed" ? "success-text" : "warning-text"}>{item.status === "removed" ? t("history.removed") : item.status === "pending" ? t("history.pending") : item.status === "running" ? t("history.runningItem") : item.status === "failed" ? t("history.failed") : t("history.skipped")}</span>
        {localizeBackendText(item.title)}</summary><code>{item.path}</code><p>{localizeBackendText(item.message)}</p></details>)}
    </article>)}
  </section>;
}

function Empty({ title, text, icon = "search" }: { title: string; text: string; icon?: string }) {
  return <div className="empty-state"><div className="empty-icon"><Icon name={icon} /></div><h2>{title}</h2><p>{text}</p></div>;
}

function LanguageSwitcher() {
  const { locale, setLocale } = useLocale();
  return <details className="language-menu">
    <summary title={t("nav.language")}><Icon name="globe" /><span>{t("nav.language")}</span></summary>
    <div className="language-popover">{LOCALE_OPTIONS.map(option => <button
      key={option.value} type="button" className={option.value === locale ? "active" : ""}
      onClick={event => {
        setLocale(option.value);
        event.currentTarget.closest("details")?.removeAttribute("open");
      }}>
      <span className="language-native">{option.native}</span>
      <span className="language-english">{option.english}</span>
      {option.value === locale && <Icon name="check" />}
    </button>)}</div>
  </details>;
}

export default function App() {
  useLocale();
  const [view, setView] = useState<View>("quick");
  const [boot, setBoot] = useState<Bootstrap | null>(null);
  const [tasks, setTasks] = useState<Record<ScanMode, ScanTask>>({
    quick: emptyTask(), projects: emptyTask(), installers: emptyTask(), full: emptyTask(),
  });
  const tasksRef = useRef(tasks);
  const [operation, setOperation] = useState<"prepare" | "clean" | "save" | "recheck" | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [preview, setPreview] = useState<Preview | null>(null);
  const [validation, setValidation] = useState<ValidationProgress | null>(null);
  const [validationCancelling, setValidationCancelling] = useState(false);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [rootsText, setRootsText] = useState("");
  const [excludedText, setExcludedText] = useState("");
  const busy = useRef(false);
  const controls = useRef(new Map<string, api.DemoControl>());
  const validationScanId = useRef("");
  const detail = useRef<HTMLElement>(null);
  const currentMode = isScan(view) ? view : "quick";
  const task = tasks[currentMode];
  const { report, progress, query, category, sort, selected, activeId } = task;
  const scanning = isScan(view) && taskActive(task);
  const anyScanning = Object.values(tasks).some(taskActive);
  const cancelling = task.state === "cancelling";
  const rootInput = tasks.full.root;
  const setRootInput = (root: string) => patch("full", { root });
  const setQuery = (query: string) => patch(currentMode, { query });
  const setCategory = (category: string) => patch(currentMode, { category });
  const setSort = (sort: string) => patch(currentMode, { sort });
  const setActiveId = (activeId: string) => patch(currentMode, { activeId });
  const setSelected = (value: Set<string> | ((value: Set<string>) => Set<string>)) =>
    patch(currentMode, current => ({ ...current, selected: typeof value === "function" ? value(current.selected) : value }));

  function patch(mode: ScanMode, change: Partial<ScanTask> | ((task: ScanTask) => ScanTask)) {
    const current = tasksRef.current[mode];
    const next = typeof change === "function" ? change(current) : { ...current, ...change };
    tasksRef.current = { ...tasksRef.current, [mode]: next };
    setTasks(tasksRef.current);
  }
  function receive(p: Progress) { patch(p.mode, current => mergeProgress(current, p)); }
  useEffect(() => {
    let alive = true;
    api.bootstrap().then(data => {
      if (!alive) return;
      setBoot(data); setHistory(data.history);
      setRootsText(data.settings.projectRoots.join("\n"));
      setExcludedText(data.settings.excludedPaths.join("\n"));
      setRootInput(data.home);
    }).catch(e => alive && setError(String(e)));
    const listener = api.onProgress(p => { if (alive) receive(p); });
    listener.catch(e => alive && setError(t("error.progress", { error: String(e) })));
    const validationListener = api.onValidationProgress(p => {
      if (alive && validationScanId.current === p.scanId) setValidation(p);
    });
    validationListener.catch(e => alive && setError(t("error.progress", { error: String(e) })));
    return () => {
      alive = false;
      void listener.then(stop => stop(), () => {});
      void validationListener.then(stop => stop(), () => {});
    };
  }, []);
  useEffect(() => { detail.current?.scrollTo(0, 0); }, [view, activeId]);

  function navigate(next: View) {
    if (busy.current) return;
    setView(next); setError(""); setNotice("");
  }
  async function scan(mode: ScanMode, root: string | null = null) {
    if (busy.current || taskActive(tasksRef.current[mode])) return;
    setError(""); setNotice("");
    const scanId = `scan-${crypto.randomUUID()}`;
    const control = { paused: false, cancelled: false };
    controls.current.set(scanId, control);
    patch(mode, { ...emptyTask(), id: scanId, state: "running", root: root || tasksRef.current[mode].root,
      progress: { scanId, mode, scannedEntries: 0, unreadableEntries: 0, currentPath: root || t("scan.preparing") } });
    try {
      const next = await api.scan(mode, scanId, root, control, receive);
      if (tasksRef.current[mode].id !== scanId) return;
      patch(mode, current => ({ ...current, report: next, candidates: next.candidates,
        root: next.root, state: "done", controlPending: false,
        activeId: next.candidates.some(i => i.id === current.activeId) ? current.activeId : next.candidates[0]?.id ?? "",
        notice: next.cancelled ? t("scan.cancelledNotice") : "" }));
    } catch (e) {
      if (tasksRef.current[mode].id === scanId) patch(mode, { error: String(e), state: "done", controlPending: false });
    } finally { controls.current.delete(scanId); }
  }
  async function controlTask(action: "pause" | "resume" | "cancel") {
    const mode = currentMode;
    const current = tasksRef.current[mode];
    if (!taskActive(current) || current.controlPending) return;
    patch(mode, { controlPending: true });
    try {
      if (api.native) {
        const accepted = await api.call<boolean>(`${action}_scan`, { scanId: current.id });
        if (!accepted) throw new Error(t("scan.controlRejected"));
        if (action === "pause") {
          const candidates = await api.call<Candidate[]>("scan_candidates", { scanId: current.id });
          if (tasksRef.current[mode].id === current.id) {
            patch(mode, task => ({
              ...task,
              candidates,
              activeId: candidates.some(item => item.id === task.activeId) ? task.activeId : candidates[0]?.id ?? "",
            }));
          }
        }
      } else {
        const control = controls.current.get(current.id);
        if (!control) return;
        if (action === "pause") control.paused = true;
        if (action === "resume") control.paused = false;
        if (action === "cancel") { control.cancelled = true; control.paused = false; }
      }
      if (tasksRef.current[mode].id === current.id && taskActive(tasksRef.current[mode])) {
        patch(mode, { state: action === "pause" ? "paused" : action === "resume" ? "running" : "cancelling" });
      }
    } catch (e) { patch(mode, { error: String(e) }); }
    finally { if (tasksRef.current[mode].id === current.id) patch(mode, { controlPending: false }); }
  }
  async function recheck(item: Candidate, removeManual = false) {
    if (!report || busy.current) return;
    const mode = currentMode;
    busy.current = true; setOperation("recheck"); setError("");
    try {
      const updated = await api.recheck(report.scanId, item, removeManual);
      patch(mode, current => ({ ...current,
        report: current.report ? { ...current.report, candidates: current.report.candidates.map(i => i.id === updated.id ? updated : i) } : null,
        candidates: current.candidates.map(i => i.id === updated.id ? updated : i),
        selected: new Set([...current.selected].filter(id => id !== item.id)),
      }));
      const data = await api.bootstrap();
      setBoot(data); setExcludedText(data.settings.excludedPaths.join("\n"));
      setNotice(removeManual ? t("notice.unprotected") : t("notice.rechecked"));
    } catch (e) { setError(String(e)); }
    finally { setOperation(null); busy.current = false; }
  }
  function removeRule(rule: string) {
    if (anyScanning) setNotice(t("settings.endScans"));
    else void save(undefined, rule);
  }
  async function prepare() {
    const mode = currentMode;
    const current = tasksRef.current[mode];
    const scanId = current.report?.scanId || current.id;
    if (!scanId || busy.current || !chosen.length || !["paused", "done"].includes(current.state)) return;
    busy.current = true; setOperation("prepare"); setError(""); setValidationCancelling(false);
    try {
      if (current.state === "paused") {
        if (api.native) {
          const accepted = await api.call<boolean>("cancel_scan", { scanId });
          if (!accepted) throw new Error(t("scan.controlRejected"));
        } else {
          const control = controls.current.get(scanId);
          if (control) { control.cancelled = true; control.paused = false; }
        }
        patch(mode, { state: "cancelling" });
        await new Promise<void>((resolve, reject) => {
          const deadline = Date.now() + 30_000;
          const wait = () => {
            if (tasksRef.current[mode].id !== scanId || !taskActive(tasksRef.current[mode])) resolve();
            else if (Date.now() >= deadline) reject(new Error(t("scan.controlRejected")));
            else setTimeout(wait, 30);
          };
          wait();
        });
      }
      validationScanId.current = scanId;
      setValidation({ scanId, completed: 0, total: chosen.length, currentPath: "" });
      setPreview(api.native ? await api.call("prepare_cleanup", { scanId, candidateIds: chosen.map(i => i.id) }) :
        { token: "demo", items: chosen, estimatedBytes: chosen.reduce((sum, item) => sum + item.bytes, 0) });
    } catch (e) {
      const message = localizeBackendText(String(e));
      if (message === t("validation.cancelled")) setNotice(message);
      else setError(t("error.prepare", { error: message }));
    }
    finally {
      validationScanId.current = "";
      setValidation(null); setValidationCancelling(false); setOperation(null); busy.current = false;
    }
  }
  async function cancelPreparation() {
    if (operation !== "prepare" || !validationScanId.current || validationCancelling) return;
    setValidationCancelling(true);
    try {
      if (!api.native) {
        setValidation(null);
        return;
      }
      await api.call<boolean>("cancel_prepare_cleanup", { scanId: validationScanId.current });
    } catch (e) {
      setError(String(e));
      setValidationCancelling(false);
    }
  }
  async function clean() {
    if (!api.native || !preview || busy.current) return;
    busy.current = true; setOperation("clean"); setError("");
    try {
      const entry = await api.call<HistoryEntry>("execute_cleanup", { token: preview.token });
      setHistory(current => [entry, ...current.filter(h => h.id !== entry.id)]);
      patch(currentMode, current => ({ ...emptyTask(), root: current.root }));
      setView("history"); setPreview(null);
      if (entry.availableAfter !== null) setBoot(current => current ? {
        ...current, disk: { ...current.disk, availableBytes: entry.availableAfter! },
      } : current);
    } catch (e) { setError(String(e)); setPreview(null); }
    finally { setOperation(null); busy.current = false; }
  }
  async function save(protectPath?: string, removePath?: string) {
    if (busy.current) return;
    busy.current = true; setOperation("save"); setError("");
    try {
      const result = await api.saveSettings({
        projectRoots: parsePaths(rootsText),
        excludedPaths: [...new Set([...parsePaths(excludedText), ...(protectPath ? [protectPath] : [])])].filter(p => p !== removePath),
      });
      const settings = result.settings;
      setRootsText(settings.projectRoots.join("\n")); setExcludedText(settings.excludedPaths.join("\n"));
      setBoot(current => current ? { ...current, settings } : current);
      for (const mode of ["quick", "projects", "installers"] as const) {
        patch(mode, current => {
          const candidates = current.candidates.map(item => applySettings(item, settings));
          return {
            ...current,
            candidates,
            report: current.report ? { ...current.report, candidates } : null,
            selected: new Set([...current.selected].filter(id =>
              candidates.some(item => item.id === id && item.cleanable && item.complete))),
          };
        });
      }
      setNotice(t("settings.saved"));
    } catch (e) { setError(String(e)); }
    finally { busy.current = false; setOperation(null); }
  }
  async function reveal(item: Candidate) {
    if (!report) return;
    if (!api.native) { setNotice(t("notice.browserFinder")); return; }
    try { await api.call("reveal_item", { scanId: report.scanId, candidateId: item.id }); }
    catch (e) { setError(String(e)); }
  }

  const metadata = sections.find(s => s.id === view)!;
  const disk = report?.disk ?? boot?.disk ?? null;
  const health = diskHealth(disk);
  const items = useMemo(() => visibleItems(task.candidates, query, category, sort), [task.candidates, query, category, sort]);
  const chosen = selection(task.candidates, selected);
  const active = task.candidates.find(i => i.id === activeId);
  const categories = ["", ...new Set(task.candidates.map(i => i.category))];
  const total = task.candidates.filter(i => i.cleanable && i.complete).reduce((n, i) => n + i.bytes, 0);
  const largest = Math.max(1, ...task.candidates.map(i => i.bytes));
  const full = view === "full";
  const locked = operation !== null || scanning;
  const selectionLocked = operation !== null || task.state === "running" || task.state === "cancelling";
  const canPreview = task.state === "paused" || task.state === "done";

  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand"><div className="brand-mark"><Icon name="disk" /></div><strong>CDisk<span>{t("brand.tagline")}</span></strong></div>
      <nav aria-label={t("nav.label")}>{sections.map((section, index) => <button
        key={section.id} className={`nav-item ${view === section.id ? "active" : ""} ${index === 4 ? "nav-divider" : ""}`}
        onClick={() => navigate(section.id)} disabled={operation !== null} aria-current={view === section.id ? "page" : undefined}>
        <Icon name={section.icon} /><span>{t(section.labelKey)}</span>
        {isScan(section.id) && taskActive(tasks[section.id]) ?
          <small className="task-indicator">{tasks[section.id].state === "paused" ? t("task.paused") : tasks[section.id].state === "cancelling" ? t("task.cancelling") : t("task.scanning")}</small> : view === section.id && <i />}
      </button>)}</nav>
      <div className="sidebar-bottom"><LanguageSwitcher /><div className="local-note"><Icon name="shield" /><span>{t("nav.localOnly")}<br /><small>{t("nav.localNote")}</small></span></div></div>
      <small className="version">CDisk 0.5.2</small>
    </aside>
    <main>
      <header className="page-header"><div><h1>{t(metadata.labelKey)}</h1><p>{t(metadata.detailKey)}</p></div>
        {isScan(view) && <button className="primary" disabled={locked || !boot}
          onClick={() => void scan(view, full ? rootInput : null)}>
          <Icon name={report ? "refresh" : "search"} />{scanning ? task.state === "paused" ? t("action.scanPaused") : t("action.scanning") : report ? t("action.rescan") : t("action.startScan")}
        </button>}
      </header>
      {!api.native && <div className="banner demo">{t("banner.demo")}</div>}
      {error && <div className="banner error" role="alert">{localizeBackendText(error)}<button onClick={() => setError("")} aria-label={t("action.closeError")}><Icon name="close" /></button></div>}
      {notice && <div className="banner notice" role="status">{localizeBackendText(notice)}<button onClick={() => setNotice("")} aria-label={t("action.closeNotice")}><Icon name="close" /></button></div>}
      {isScan(view) && task.error && <div className="banner error" role="alert">{localizeBackendText(task.error)}<button onClick={() => patch(currentMode, { error: "" })} aria-label={t("action.closeError")}><Icon name="close" /></button></div>}
      {isScan(view) && task.notice && <div className="banner notice" role="status">{task.notice}</div>}
      {operation === "prepare" && validation && <section className="validation-progress" aria-live="polite">
        <div className="spinner" />
        <div><strong>{t("validation.progress", { completed: validation.completed, total: validation.total })}</strong>
          <div className="validation-track"><i style={{ width: `${validation.total ? validation.completed / validation.total * 100 : 0}%` }} /></div>
          <code title={validation.currentPath}>{validation.currentPath || t("validation.preparing")}</code>
        </div>
        <button onClick={() => void cancelPreparation()} disabled={validationCancelling}>
          {validationCancelling ? t("validation.cancelling") : t("validation.cancel")}
        </button>
      </section>}
      {isScan(view) && <>
        <section className="disk-summary" aria-label={t("disk.summary")}>
          <div><Icon name="disk" /><div><strong>Macintosh HD <span>Data</span></strong><p className={health.tone}>{health.label}</p></div></div>
          <div className="disk-capacity"><strong>{disk ? bytes(disk.availableBytes) : "—"}<span> {t("disk.available")}</span></strong>
            <div className="capacity-track"><i style={{ width: `${health.percent}%` }} /></div>
            <small>{t("disk.total", { size: disk ? bytes(disk.totalBytes) : "—" })}</small></div>
          <div className="summary-result"><strong>{report || task.candidates.length ? full ? task.candidates.length : bytes(total) : "—"}</strong>
            <span>{full ? t("disk.directChildren", { count: task.candidates.length }) : t("disk.reviewable")}</span></div>
        </section>
        {full && <form className="location-bar" onSubmit={e => { e.preventDefault(); void scan("full", rootInput); }}>
          <button type="button" aria-label={t("action.parentFolder")} disabled={locked || !report?.parent} onClick={() => void scan("full", report!.parent)}>↑</button>
          <label className="sr-only" htmlFor="scan-root">{t("scan.folder")}</label><input id="scan-root" value={rootInput} onChange={e => setRootInput(e.target.value)} disabled={locked} spellCheck={false} />
          <button type="button" disabled={locked} onClick={() => void scan("full", boot?.home ?? null)}>{t("action.homeFolder")}</button>
          <button type="button" disabled={locked} onClick={() => void scan("full", "/System/Volumes/Data")}>{t("action.wholeDisk")}</button>
          <button type="submit" disabled={locked}>{t("action.analyzeFolder")}</button>
        </form>}
        {scanning && progress && <section className="scan-progress" aria-live="polite">
          <div className={`spinner ${task.state === "paused" ? "paused" : ""}`} /><div><strong>{task.state === "paused" ? t("scan.pausedPrefix") : ""}{t("scan.checked", { count: formatCount(progress.scannedEntries) })}</strong>
            <code title={progress.currentPath}>{progress.currentPath}</code></div>
          <small>{t("scan.unreadable", { count: formatCount(progress.unreadableEntries) })}</small>
          <button onClick={() => void controlTask(task.state === "paused" ? "resume" : "pause")} disabled={cancelling || task.controlPending}>
            {task.state === "paused" ? t("action.resume") : t("action.pause")}
          </button>
          <button onClick={() => void controlTask("cancel")} disabled={cancelling || task.controlPending}>{cancelling ? t("action.cancelling") : t("action.cancel")}</button>
        </section>}
        {report && <div className="scan-meta">
          <span>{t("scan.summary", { state: report.cancelled ? t("scan.partial") : t("scan.complete"), seconds: (report.elapsedMs / 1000).toFixed(1), count: formatCount(report.scannedEntries) })}</span>
          <span>{report.unreadableEntries > 0 ? t("scan.readErrors", { count: formatCount(report.unreadableEntries) }) : t("scan.noReadErrors")}
            {report.skippedEntries > 0 && ` · ${t("scan.skipped", { count: formatCount(report.skippedEntries) })}`}{report.truncated && ` · ${t("scan.truncated")}`}</span>
        </div>}
        <div className="workspace">
          <section className="candidate-panel">
            <div className="list-toolbar">
              <div className="search-field"><Icon name="search" /><input aria-label={t("search.label")} placeholder={t("search.placeholder")} value={query} onChange={e => setQuery(e.target.value)} /></div>
              <select aria-label={t("sort.label")} value={sort} onChange={e => setSort(e.target.value)}>
                <option value="size">{t("sort.size")}</option><option value="name">{t("sort.name")}</option><option value="recent">{t("sort.recent")}</option>
              </select>
            </div>
            <div className="category-tabs" role="group" aria-label={t("filter.category")}>{categories.map(c =>
              <button key={c || "all"} aria-pressed={category === c} onClick={() => setCategory(c)}>{c ? categoryLabel(c) : t("filter.all")}</button>)}</div>
            {!full && report && <div className="selection-tools">
              <button disabled={selectionLocked} onClick={() => setSelected(new Set(items.filter(i => i.cleanable && i.complete && i.recommended).map(i => i.id)))}>{t("action.selectRecommended")}</button>
              <button disabled={selectionLocked} onClick={() => setSelected(new Set())}>{t("action.clearSelection")}</button>
              <span>{t("selection.recentKept")}</span>
            </div>}
            <div className="candidate-list" tabIndex={0} aria-label={t("list.label")}>
              {!report && !task.candidates.length && <Empty title={scanning ? task.state === "paused" ? t("empty.scanPaused") : t("empty.scanning") : t("empty.start")} text={scanning ? t("empty.scanningText") : full ? t("empty.fullText") : t("empty.startText")} />}
              {report && !items.length && <Empty title={query ? t("empty.noMatch") : t("empty.noCandidates")} text={t("empty.tryAnother")} />}
              {items.map(item => <div className={`candidate-row ${activeId === item.id ? "active" : ""}`} key={item.id}>
                {!full && <input type="checkbox" aria-label={t("list.selectItem", { title: localizeBackendText(item.title) })} checked={selected.has(item.id)}
                  disabled={selectionLocked || !item.cleanable || !item.complete} onChange={() => setSelected(current => {
                    const next = new Set(current); if (next.has(item.id)) next.delete(item.id); else next.add(item.id); return next;
                  })} />}
                <button className="candidate-open" onClick={() => setActiveId(item.id)}
                  onDoubleClick={() => { if (full && item.isDir && !locked) void scan("full", item.path); }}>
                  <div className={`item-icon ${item.cleanable ? "" : "muted"}`}><Icon name={item.isDir ? "folder" : "file"} /></div>
                  <div className="item-name"><strong>{localizeBackendText(item.title)}</strong><code title={item.path}>{item.path.replace(boot?.home ?? "/Users/demo", "~")}</code>
                    <div className="size-track"><i style={{ width: `${Math.max(1, item.bytes / largest * 100)}%` }} /></div>
                  </div>
                  <div className="item-stat"><strong>{bytes(item.bytes)}</strong><span className={`badge ${item.status}`}>{statusLabel(item.status)}</span></div>
                  <Icon name="arrow" />
                </button>
              </div>)}
            </div>
          </section>
          <aside className="detail-panel" ref={detail} tabIndex={0} aria-label={t("detail.label")}>
            {!active ? <Empty icon="shield" title={t("detail.emptyTitle")} text={t("detail.emptyText")} /> : <>
              <div className="detail-top"><div className="detail-icon"><Icon name={active.isDir ? "folder" : "file"} /></div><span className={`badge ${active.status}`}>{statusLabel(active.status)}</span></div>
              <h2>{localizeBackendText(active.title)}</h2>
              <div className={`reason ${active.cleanable ? "review" : "protected"}`}><Icon name="shield" /><p>{localizeBackendText(active.reason)}</p></div>
              <p>{localizeBackendText(active.description)}</p>
              <div className="detail-size">{bytes(active.bytes)}<small>{active.status === "partial" ? t("detail.partialSize") : t("detail.scanSize")}</small></div>
              <dl><div><dt>{t("detail.type")}</dt><dd>{categoryLabel(active.category)}</dd></div><div><dt>{t("detail.recent")}</dt><dd>{age(active.modifiedAt)}</dd></div><div><dt>{t("detail.entries")}</dt><dd>{formatCount(active.entries)}</dd></div></dl>
              <label className="path-label">{t("detail.fullPath")}</label><code className="path-block">{active.path}</code>
              <div className="detail-actions">{full && active.isDir && <button disabled={locked} className="primary" onClick={() => void scan("full", active.path)}>{t("action.enterFolder")}<Icon name="arrow" /></button>}
                <button disabled={locked || !report} onClick={() => void reveal(active)}>{t("action.revealFinder")}</button>
                {!full && <button disabled={locked || !report} onClick={() => void recheck(active)}>{t("action.recheck")}</button>}
                {active.protection === "manual" && active.excludedBy ?
                  <div className="manual-rule"><small>{t("detail.manualRule", { rule: active.excludedBy })}</small>
                    <button disabled={locked || !report} onClick={() => void recheck(active, true)}>{t("action.removeProtection")}</button></div> :
                  <button disabled={operation !== null || anyScanning || !report} onClick={() => void save(active.path)}>{t("action.addProtection")}</button>}
                {active.protection === "mounted" && <small>{t("detail.mountedHint")}</small>}
              </div>
              <div className="detail-footnote">{t("detail.footnote")}</div>
            </>}
          </aside>
        </div>
        <footer className="action-bar"><div>{full ? <><Icon name="shield" /><span>{t("selection.readOnly")}</span></> :
          <><span>{t("selection.count", { count: chosen.length })}</span><strong>{bytes(chosen.reduce((sum, i) => sum + i.bytes, 0))}</strong></>}</div>
          {!full && <button className="primary" disabled={operation !== null || !canPreview || !chosen.length} onClick={() => void prepare()}>
            {operation === "prepare" ? t("action.revalidating") : t("action.previewCleanup")}<Icon name="arrow" /></button>}
        </footer>
      </>}
      {view === "history" && <History entries={history} />}
      {view === "settings" && <section className="page-scroll settings-page">
        {anyScanning && <p className="warning-text">{t("settings.scanningWarning")}</p>}
        <div className="settings-section"><h2>{t("settings.projectRoots")}</h2><p>{t("settings.projectRootsHelp")}</p>
          <textarea aria-label={t("settings.projectRoots")} value={rootsText} onChange={e => setRootsText(e.target.value)} placeholder={"~/repos\n~/Projects"} disabled={locked} spellCheck={false} /></div>
        <div className="settings-section"><h2>{t("settings.protectedPaths")}</h2><p>{t("settings.protectedPathsHelp")}</p>
          <textarea aria-label={t("settings.protectedPaths")} value={excludedText} onChange={e => setExcludedText(e.target.value)} placeholder="~/Projects/important-project" disabled={locked} spellCheck={false} /></div>
        <div className="saved-rules">{boot?.settings.excludedPaths.map(rule => <div key={rule}><code>{rule}</code>
          <button disabled={operation !== null || anyScanning} onClick={() => removeRule(rule)} aria-label={t("settings.removeRule", { rule })}>{t("action.remove")}</button></div>)}</div>
        <button className="primary" disabled={operation !== null || anyScanning || !boot} onClick={() => void save()}>{operation === "save" ? t("action.saving") : t("action.save")}</button>
        <div className="settings-section limits"><h2>{t("settings.limits")}</h2><p>{t("settings.limitsText")}</p></div>
      </section>}
    </main>
    {preview && <Confirm preview={preview} busy={operation === "clean"} onClose={() => setPreview(null)} onConfirm={() => void clean()} />}
  </div>;
}
