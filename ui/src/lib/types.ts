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
}

export interface Tests {
	target: string;
	total: number;
	truncated: boolean;
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
}

export interface Applied {
	lane: Lane;
	source: string;
	reason: string;
}

export interface Classification {
	lane: Lane;
	applied: Applied[];
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
	policy: {
		default?: Lane;
		auditRate?: number;
		minRecord?: number;
		rules?: LaneRule[];
		heldOut?: CheckCommand | null;
		taste?: { command: string[] } | null;
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

export interface EscalationItem {
	request: EscalationRequest;
	decision: { action: string; decision: string; by: string; reason: string; at: number } | null;
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
