// The shapes the API answers with. They mirror the Rust types (and the JSON
// Schemas in schemas/) field for field.

export type Lane = 'auto-merge' | 'judge' | 'human' | 'blocked';

export interface Status {
	version: string;
	root: string;
	name: string;
	branch: string;
	head: { sha: string; subject: string; author: string; at: number };
	dirty: string[];
	map: {
		commit: string;
		components: number;
		files: number;
		symbols: number;
		edges: number;
		tests: number;
		diagnostics: number;
		componentDirs: Record<string, string>;
	};
	mapMeta: { version: number; rebuilt: boolean; buildMs: number; watching: boolean };
	config: {
		exists: boolean;
		components: number;
		contracts: number;
		rules: number;
		lanes: boolean;
		environment: boolean;
	};
	paths: {
		outcomes: string;
		audit: string;
		escalations: string;
		keys: string;
		evidence: string | null;
	};
}

export interface GitRef {
	name: string;
	sha: string;
	at: number;
	subject: string;
}

export interface Refs {
	refs: GitRef[];
	commits: GitRef[];
	default: string | null;
}

// ---- The map

export interface SymbolRef {
	id: string;
	name: string;
	kind: string;
	component: string | null;
	public: boolean;
	file: string | null;
	line: number | null;
}

export interface GraphComponent {
	id: string;
	kind: string;
	roots: string[];
	owners: string[];
	labels: string[];
	packageName: string | null;
	files: number;
	lines: number;
	publicSymbols: number;
	externals: string[];
}

export interface GraphEdge {
	from: string;
	to: string;
	count: number;
	kinds: Record<string, number>;
}

export interface Graph {
	components: GraphComponent[];
	edges: GraphEdge[];
	events: { name: string; publishers: string[]; consumers: string[] }[];
	externals: { id: string; vendor?: string; category?: string; egress?: string[] }[];
	rules: unknown[];
	diagnostics: { file?: string; message: string; severity?: string }[];
}

export interface ComponentInfo {
	id: string;
	kind: string;
	roots: string[];
	owners: string[];
	labels: string[];
	packageName: string | null;
	files: number;
	publicSymbols: number;
	public: SymbolRef[];
	uses: Record<string, number>;
	usedBy: Record<string, number>;
	externals: string[];
	eventsPublished: string[];
	eventsConsumed: string[];
}

export interface FileInfo {
	path: string;
	component: string | null;
	module: string | null;
	isTest: boolean;
	lines: number;
	symbols: SymbolRef[];
	imports: string[];
	importedBy: string[];
	diagnostics: string[];
}

export interface FileRow {
	path: string;
	component: string | null;
	language: string;
	isTest: boolean;
	lines: number;
}

export interface Found {
	query: string;
	total: number;
	truncated: boolean;
	symbols: (SymbolRef & { matchedWords: number; matchedFields?: string[] })[];
	files: string[];
	hint?: string | null;
}

export interface SymbolInfo {
	symbol: SymbolRef;
	shape: unknown | null;
	invariants: string[];
	uses: number;
	usedBy: number;
	tests: number;
}

export interface Link {
	id: string;
	name: string;
	component: string | null;
	kind: string;
	via: string;
	depth: number;
	file: string | null;
	line: number | null;
	confidence: string;
}

export interface Walk {
	target: string;
	resolved: string[];
	depth: number;
	total: number;
	truncated: boolean;
	components: Record<string, number>;
	links: Link[];
}

export interface TestRef {
	file: string;
	component: string | null;
	cases: number;
	skipped: number;
	/** How the link is known, strongest first: coverage saw the test run it, the test refers to it and it ran, no coverage, or it never ran. */
	link?: 'ran' | 'static-ran' | 'static' | 'static-not-run';
}

export interface Tests {
	target: string;
	total: number;
	truncated: boolean;
	/** The target's lines in the coverage, when there is some. */
	coverage?: { executable: number; ran: number } | null;
	tests: TestRef[];
}

export interface ImpactSite {
	file: string;
	line: number;
	component: string | null;
	kind: string;
	uses: string;
	depth: number;
	test: boolean;
}

export interface Impact {
	target: string;
	resolved: string[];
	change: string;
	total: number;
	truncated: boolean;
	components: Record<string, number>;
	sites: ImpactSite[];
	tests: TestRef[];
	notes: string[];
}

export interface Invariants {
	target: string;
	contracts: { symbol: SymbolRef; invariants: string[] }[];
}

// ---- Reports

export type ChangeKind =
	| 'additive'
	| 'breaking'
	| 'internal'
	| 'dependency'
	| 'config'
	| 'test'
	| 'security-sensitive';

export interface Location {
	file: string;
	lines: [number, number];
	side: 'base' | 'head';
}

export interface SemanticChange {
	id: string;
	kind: ChangeKind;
	subkind: string;
	level: string;
	subject: string;
	component?: string | null;
	kindLabel: string;
	title: string;
	whyItMatters: string;
	hints: {
		labels: string[];
		blastRadius: number;
		novelty: string[];
		confidence: string;
		rulesOfTheGame: boolean;
		intentMismatch: boolean;
		needsPerson: boolean;
		/** Other components that depend on what the row changed. */
		dependentComponents?: number;
		/** Whether tests or traces reach the files the row changes. */
		tested?: 'tested' | 'traced' | 'untested';
		/** The row's changed executable lines and how many ran in tests. */
		coverage?: { changed: number; ran: number } | null;
	};
	locations: Location[];
	stats?: { linesAdded: number; linesRemoved: number; files: number } | null;
}

export interface SemanticReport {
	schemaVersion: number;
	base: string;
	head: string;
	summary: {
		meaningChanges: number;
		needsAttention: number;
		secrets: number;
		newRuleViolations: number;
		intentMismatches: number;
	};
	changes: SemanticChange[];
	intentCheck?: {
		stated: string;
		touches: string[];
		contracts: string[];
		externals: string[];
		mismatches: { changeId: string; reason: string }[];
	} | null;
	ruleViolations: SemanticChange[];
	structure: { movedFiles: { from: string; to: string }[]; formattingOnly: string[] };
	textStats: { files: number; linesAdded: number; linesRemoved: number };
	mapDiagnostics: { file?: string; message: string }[];
	/** Which changed lines tests ran, when coverage of the head was given. */
	coverage?: {
		changed: number;
		ran: number;
		notRun: Location[];
		notMeasured: string[];
		/** `command` is the run's, for recorded coverage. */
		sources: { format: string; path: string; origin: string; manifest?: string | null; command?: string | null }[];
		notes?: string[];
	} | null;
}

export interface Applied {
	lane: Lane;
	/** `default`, `policy`, `input`, `floor`, `model`, `record` or `audit`. */
	source: string;
	reason: string;
	/** The ids of the report rows behind it. */
	rows?: string[];
}

/** A feature that pushed the learned risk up. */
export interface LearnedFactor {
	feature: string;
	value: number;
	contribution: number;
	/** In plain words: "2 breaking rows". */
	text: string;
}

/** The learned lane model's view of a change; it can only raise a lane. */
export interface LearnedRisk {
	/** `applied`, `missing`, `stale` or `invalid`. */
	status: string;
	model?: string | null;
	risk?: number | null;
	judgeAt: number;
	humanAt: number;
	raised: boolean;
	factors: LearnedFactor[];
	/** Why a missing, stale or invalid model was ignored. */
	note?: string | null;
}

export interface Classification {
	lane: Lane;
	applied: Applied[];
	learned?: LearnedRisk;
}

export interface ReportAnswer {
	report: SemanticReport;
	markdown: string;
	checklist: string[];
	classification?: Classification;
	notes?: string[];
	baseSha?: string;
	headSha?: string;
}

// ---- Lanes

export interface TestRun {
	commit: string;
	image: string;
	setup?: string | null;
	command: string;
	exitCode: number;
	outputTail: string;
	manifest?: string | null;
}

export interface AgentSetup {
	tool: string;
	model: string;
	config: string;
	team: string;
}

export interface Submission {
	schema: number;
	base: string;
	head: string;
	intent?: string | null;
	report: SemanticReport;
	changedFiles: string[];
	evidence: TestRun[];
	scope?: { task: string; rights: string[]; attenuations: string[] } | null;
	escalations: string[];
	approvals: { row: string; by: string }[];
	agent: AgentSetup;
}

export interface Step {
	name: string;
	status: 'passed' | 'failed' | 'skipped' | 'concern';
	details: string[];
}

export interface Judgment {
	verdict: 'approve' | 'reject' | 'escalate';
	lane: Lane;
	steps: Step[];
	reasons: string[];
	judge: string;
	/** The configuration from lanes.judges that judged, when there are any. */
	rotation?: { config: string; index: number; of: number; forced: boolean } | null;
	/** The held-out step in counts; it never names a check. */
	heldOut?: { suite?: string; selected: number; ran: number; passed: number } | null;
}

/** What the held-out checks did, for people only (`<id>.held-out.json`). */
export interface HeldOutReport {
	suite: string;
	judge: string;
	/** `map` is false when the head map could not be built, absent when no check needed it. */
	reach: { touched: string[]; pairs: [string, string][]; map?: boolean };
	checks: {
		id: string;
		why: 'combination' | 'subject' | 'touched' | 'always' | 'neighbour';
		with?: string[];
		status: 'passed' | 'failed' | 'not-run' | 'error';
		exitCode?: number | null;
		outputTail?: string;
	}[];
	notSelected: string[];
}

export interface JudgeConfig {
	name: string;
	taste?: { command: string[] } | null;
	heldOutLimit?: number | null;
}

export interface CheckCommand {
	command: string;
	image?: string | null;
	setup?: string | null;
}

export interface LaneRule {
	lane: Lane;
	match?: 'any' | 'every';
	kinds?: string[];
	subkinds?: string[];
	components?: string[];
	labels?: string[];
}

export interface LanesPolicy {
	configured: boolean;
	judge: string;
	/** Each configuration in lanes.judges with its judge id. */
	judges?: { name: string; id: string }[];
	/** Whether `onus ui --held-out` gave a suite. */
	heldOutSuite?: boolean;
	policy: {
		default?: Lane;
		auditRate?: number;
		minRecord?: number;
		rules?: LaneRule[];
		heldOut?: CheckCommand | null;
		taste?: { command: string[] } | null;
		judges?: JudgeConfig[];
	};
}

export interface Outcome {
	at: number;
	change: string;
	agent: string;
	judge?: string | null;
	lane: Lane;
	verdict?: string | null;
	result: string;
	audited: boolean;
	missed: boolean;
	commit?: string | null;
	involved?: string[];
	note?: string | null;
}

export interface Tally {
	changes: number;
	merged: number;
	reverted: number;
	incidents: number;
	autoMerged: number;
	audited: number;
	missed: number;
}

export interface Outcomes {
	file: string;
	records: Outcome[];
	summary: {
		total: Tally;
		byAgent: Record<string, Tally>;
		byJudge: Record<string, Tally>;
		byLane: Record<string, number>;
		humanShare: number | null;
		auditMissRate: number | null;
	};
	backlog: { target: string; incidents: number }[];
	results: string[];
	ingested?: { found: number; recorded: number };
	source: { kind: 'file'; path: string } | { kind: 'branch'; ref: string; updatedAt: number | null } | { kind: 'none'; path: string };
	trend: { week: number; counts: Record<string, number> }[];
	agents: { agent: string; merged: number; recentIncidents: number; eligible: boolean; needs: number }[];
	minRecord: number;
	inFlight: {
		at: number;
		change: string;
		base: string;
		head: string;
		agent: { tool: string; model: string; config: string };
		lane: Lane;
		verdict?: string | null;
		approvals?: { row: string; by: string }[];
	}[];
}

// ---- Environments and evidence

export interface EnvSpec {
	image: string;
	setup?: string | null;
	seed?: string | null;
	evidence: string[];
	traces: string[];
	lockfiles: string[];
	egressImage: string;
}

export interface Env {
	name: string;
	container: string;
	commit: string;
	image: string;
	warmImage?: string | null;
	setup?: string | null;
	seed?: string | null;
	hosts: string[];
	secrets: string[];
	evidence: string[];
	traces: string[];
	createdAt: number;
	state?: string;
}

export interface Artifact {
	kind: 'log' | 'junit' | 'trace';
	path: string;
	sha256: string;
	size: number;
}

export interface Manifest {
	schema: number;
	commit: string;
	command: string;
	exitCode: number;
	startedAt: number;
	finishedAt: number;
	environment: {
		name: string;
		image: string;
		warmImage?: string | null;
		setup?: string | null;
		seed?: string | null;
		hosts: string[];
		secrets: string[];
	};
	artifacts: Artifact[];
	tests?: { tests: number; failures: number; errors: number; skipped: number } | null;
}

export interface Run {
	id: string;
	manifest: Manifest;
}

// ---- Scopes

export interface KeysStatus {
	dir: string;
	private: boolean;
	public: boolean;
	publicKey: string | null;
}

export interface TokenInfo {
	task: string;
	expires: number;
	rights: string[];
	attenuations: string[];
}

export interface EscalationEvidence {
	grade: number;
	kind: string;
	reference: string;
	reproduced?: boolean | null;
	run?: TestRun | null;
}

export interface EscalationRequest {
	id: string;
	task: string;
	kind: 'permission' | 'broken-test' | 'contradictory-spec' | 'impossible-task';
	scopes: string[];
	evidence: EscalationEvidence[];
	reason: string;
	blastRadius: number;
	sensitive: string[];
	at: number;
}

/** Where a request was filed in Onus Cloud (`<id>.cloud.json`). */
export interface EscalationCloud {
	url: string;
	organisation: string | null;
	repository: string;
	escalationId: string;
	webUrl: string | null;
	filedAt: number;
}

export interface EscalationItem {
	request: EscalationRequest;
	decision: { action: string; decision: string; by: string; reason: string; at: number } | null;
	cloud?: EscalationCloud | null;
	file: string;
}

export interface AuditEntry {
	seq: number;
	at: number;
	actor: string;
	action: string;
	subject: string;
	decision: string;
	reason: string;
	details: unknown;
	prev?: string;
	hash?: string;
}
