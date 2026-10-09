// onus.yaml as the form reads it (crates/onus-core/src/config.rs).

export type Lane = 'auto-merge' | 'judge' | 'human' | 'blocked';

export interface ComponentConfig {
	path: string | string[];
	kind?: 'service' | 'package' | 'module';
	owners?: string[];
	labels?: string[];
	entrypoints?: string[];
}

export interface LaneRule {
	lane: Lane;
	match?: 'any' | 'every';
	kinds?: string[];
	subkinds?: string[];
	components?: string[];
	labels?: string[];
}

export interface OnusConfig {
	version?: number;
	components?: Record<string, ComponentConfig>;
	contracts?: Record<string, { symbol: string; invariants?: string[] }>;
	rules?: { deny: { from: string; to: string } }[];
	labels?: Record<string, { sensitivity: 'low' | 'medium' | 'high' }>;
	extractors?: {
		events?: { publish?: string[]; subscribe?: string[] };
		externals?: Record<string, { vendor: string; category: string; egress?: string[]; hosts?: string[] }>;
		prisma?: { clients?: string[] };
		packs?: string[];
	};
	tests?: { globs?: string[] };
	testData?: string[];
	lanes?: {
		default?: Lane;
		rules?: LaneRule[];
		auditRate?: number;
		minRecord?: number;
		heldOut?: { command: string; image?: string; setup?: string };
		taste?: { command: string[] };
	};
	environment?: {
		image?: string;
		setup?: string;
		seed?: string;
		evidence?: string[];
		traces?: string[];
		lockfiles?: string[];
		egressImage?: string;
	};
}

/** What the editor offers to choose from (`config.schema`). */
export interface ConfigSchema {
	kinds: { name: string; meaning: string }[];
	subkinds: { name: string; kind: string; when: string }[];
	lanes: Lane[];
	sensitivities: string[];
	componentKinds: string[];
	components: { id: string; kind: string; roots: string[]; owners: string[]; labels: string[] }[];
}

export interface ConfigFile {
	path: string;
	exists: boolean;
	hash: string | null;
	text: string | null;
	parsed: OnusConfig | null;
	error: string | null;
}
