<script setup lang="ts">
import { computed, ref } from "vue";

type Section = "overview" | "namespaces" | "actions" | "access";
type IconName =
  | "activity"
  | "box"
  | "chevron"
  | "clock"
  | "copy"
  | "database"
  | "external"
  | "key"
  | "layers"
  | "lock"
  | "more"
  | "plus"
  | "search"
  | "settings"
  | "shield"
  | "trend"
  | "x";

const section = ref<Section>("overview");
const range = ref("7 days");
const repo = ref("All repositories");
const refFilter = ref("All refs");
const search = ref("");
const toast = ref("");
const showCreateToken = ref(false);
const showGrant = ref(false);
const selectedGrant = ref("production-deploy");
const tokenName = ref("");
const tokenScope = ref("Read + write");
const tokenNamespace = ref("acme/*");
const revealedToken = ref("");

const nav: { id: Section; label: string; icon: IconName }[] = [
  { id: "overview", label: "Overview", icon: "activity" },
  { id: "namespaces", label: "Namespaces", icon: "layers" },
  { id: "actions", label: "Missing actions", icon: "box" },
  { id: "access", label: "Access", icon: "key" },
];

const namespaces = [
  { name: "acme/web", actions: "1.42M", size: "96.4 GB", hits: "94.8%", trend: 2.4, seen: "12 sec ago" },
  { name: "acme/api", actions: "843K", size: "61.7 GB", hits: "91.2%", trend: 0.8, seen: "34 sec ago" },
  { name: "acme/mobile", actions: "392K", size: "38.1 GB", hits: "87.6%", trend: -1.3, seen: "2 min ago" },
  { name: "infra/tooling", actions: "228K", size: "14.8 GB", hits: "89.4%", trend: 3.1, seen: "8 min ago" },
];

const repoRows = [
  { repo: "acme/web", ref: "main", hits: "98.2%", requests: "184,291", saved: "418h", color: "#e6ad54" },
  { repo: "acme/api", ref: "main", hits: "96.7%", requests: "121,884", saved: "267h", color: "#79a6a8" },
  { repo: "acme/web", ref: "pull/*", hits: "91.4%", requests: "92,410", saved: "183h", color: "#cf8f35" },
  { repo: "acme/mobile", ref: "main", hits: "89.1%", requests: "64,028", saved: "106h", color: "#8ba67a" },
  { repo: "infra/tooling", ref: "release/*", hits: "86.8%", requests: "28,184", saved: "51h", color: "#8e7eae" },
];

const misses = [
  { action: "rustc · web_server", context: "acme/web · pull/1842", misses: 2841, cost: "9m 42s", cause: "source changed" },
  { action: "rustc · api_graphql", context: "acme/api · main", misses: 1927, cost: "7m 18s", cause: "dependency changed" },
  { action: "build.rs · openssl-sys", context: "acme/mobile · main", misses: 1612, cost: "4m 51s", cause: "environment changed" },
  { action: "rustc · integration_tests", context: "acme/web · pull/*", misses: 1204, cost: "4m 06s", cause: "source changed" },
  { action: "link · mbx", context: "infra/tooling · release/*", misses: 884, cost: "3m 22s", cause: "not cacheable" },
];

const grants = ref([
  { name: "github-actions", kind: "OIDC", identity: "token.actions.githubusercontent.com", scope: "acme/*", access: "Read + write", used: "18 sec ago", status: "active" },
  { name: "production-deploy", kind: "Token", identity: "mbx_tk_••••••••7a2f", scope: "acme/web", access: "Read + write", used: "3 hours ago", status: "active" },
  { name: "developer-read", kind: "Token", identity: "mbx_tk_••••••••1c90", scope: "*", access: "Read only", used: "Yesterday", status: "active" },
  { name: "legacy-runner", kind: "Token", identity: "mbx_tk_••••••••6bb1", scope: "acme/api", access: "Read + write", used: "24 days ago", status: "expiring" },
]);

const filteredNamespaces = computed(() => {
  const value = search.value.toLowerCase();
  return namespaces.filter((item) => item.name.includes(value));
});

const filteredRepoRows = computed(() =>
  repoRows.filter((item) =>
    (repo.value === "All repositories" || item.repo === repo.value) &&
    (refFilter.value === "All refs" || item.ref === refFilter.value),
  ),
);

const selectedGrantData = computed(() => grants.value.find((item) => item.name === selectedGrant.value));

function notify(message: string) {
  toast.value = message;
  window.setTimeout(() => {
    if (toast.value === message) toast.value = "";
  }, 2600);
}

function createToken() {
  if (!tokenName.value.trim()) return;
  const token = "mbx_tk_" + Array.from({ length: 4 }, () => Math.random().toString(36).slice(2, 10)).join("");
  grants.value.unshift({
    name: tokenName.value.trim(),
    kind: "Token",
    identity: "mbx_tk_••••••••" + token.slice(-4),
    scope: tokenNamespace.value.trim() || "acme/*",
    access: tokenScope.value,
    used: "Never",
    status: "active",
  });
  revealedToken.value = token;
}

async function copyToken() {
  await navigator.clipboard?.writeText(revealedToken.value);
  notify("Token copied to clipboard");
}

function closeTokenModal() {
  showCreateToken.value = false;
  tokenName.value = "";
  revealedToken.value = "";
  tokenScope.value = "Read + write";
  tokenNamespace.value = "acme/*";
}

function inspectGrant(name: string) {
  selectedGrant.value = name;
  showGrant.value = true;
}

function revokeGrant() {
  grants.value = grants.value.filter((grant) => grant.name !== selectedGrant.value);
  showGrant.value = false;
  notify("Grant revoked");
}
</script>

<template>
  <div class="dashboard-shell">
    <aside class="sidebar">
      <a class="brand" href="/" aria-label="mr boxington home">
        <span class="brand-mark" aria-hidden="true"><i></i><i></i><i></i></span>
        <span>mr boxington</span>
      </a>

      <div class="workspace-picker">
        <span class="workspace-avatar">AC</span>
        <span><b>Acme Engineering</b><small>Self-hosted</small></span>
        <Icon name="chevron" />
      </div>

      <nav aria-label="Dashboard navigation">
        <p class="nav-label">Monitor</p>
        <button v-for="item in nav.slice(0, 3)" :key="item.id" :class="{ active: section === item.id }" @click="section = item.id">
          <Icon :name="item.icon" />{{ item.label }}
          <span v-if="item.id === 'actions'" class="count">24</span>
        </button>
        <p class="nav-label access-label">Manage</p>
        <button :class="{ active: section === 'access' }" @click="section = 'access'">
          <Icon name="key" />Access
        </button>
        <button><Icon name="settings" />Settings</button>
      </nav>

      <div class="server-card">
        <div><span class="status-dot"></span><b>Server healthy</b></div>
        <p>v1.0.1 · us-east-1</p>
        <a href="/cache-server">View server docs <Icon name="external" /></a>
      </div>
      <button class="account"><span class="user-avatar">JD</span><span><b>Jordan Diaz</b><small>Administrator</small></span><Icon name="more" /></button>
    </aside>

    <main>
      <header class="topbar">
        <div class="breadcrumb"><span>Acme Engineering</span><i>/</i><b>{{ nav.find((item) => item.id === section)?.label }}</b></div>
        <div class="top-actions">
          <span class="live"><i></i>Live</span>
          <button class="icon-button" aria-label="Settings"><Icon name="settings" /></button>
          <button class="avatar">JD</button>
        </div>
      </header>

      <div class="content">
        <template v-if="section === 'overview'">
          <div class="page-heading">
            <div><p class="eyebrow">Cache operations</p><h1>Good morning, Jordan.</h1><p>Here’s how your build cache is performing.</p></div>
            <label class="select-wrap"><Icon name="clock" /><select v-model="range"><option>24 hours</option><option>7 days</option><option>30 days</option></select><Icon name="chevron" /></label>
          </div>

          <section class="metrics" aria-label="Cache metrics">
            <article><span class="metric-icon amber"><Icon name="trend" /></span><p>Hit rate</p><strong>93.6%</strong><small class="up">↑ 1.8% <i>vs previous period</i></small></article>
            <article><span class="metric-icon teal"><Icon name="activity" /></span><p>Requests</p><strong>482,910</strong><small class="up">↑ 12.4% <i>vs previous period</i></small></article>
            <article><span class="metric-icon violet"><Icon name="clock" /></span><p>Compute saved</p><strong>1,042h</strong><small class="up">↑ 8.1% <i>vs previous period</i></small></article>
            <article><span class="metric-icon green"><Icon name="database" /></span><p>Storage used</p><strong>211 GB</strong><small><i>of 500 GB</i></small><span class="storage-bar"><i></i></span></article>
          </section>

          <section class="panel chart-panel">
            <div class="panel-heading">
              <div><h2>Cache performance</h2><p>Hits and misses across all namespaces</p></div>
              <div class="legend"><span><i class="hit-dot"></i>Hits</span><span><i class="miss-dot"></i>Misses</span></div>
            </div>
            <div class="chart">
              <div class="y-axis"><span>100%</span><span>75%</span><span>50%</span><span>25%</span><span>0%</span></div>
              <div class="plot">
                <i v-for="n in 5" :key="n" class="gridline"></i>
                <svg viewBox="0 0 900 210" preserveAspectRatio="none" aria-label="Hit rate trend over seven days">
                  <defs><linearGradient id="area" x1="0" y1="0" x2="0" y2="1"><stop stop-color="#e6ad54" stop-opacity=".2"/><stop offset="1" stop-color="#e6ad54" stop-opacity="0"/></linearGradient></defs>
                  <path d="M0 186 C68 172 105 139 150 150 S245 114 300 126 S393 78 450 92 S534 54 600 66 S688 42 750 48 S835 29 900 37 L900 210 L0 210Z" fill="url(#area)" />
                  <path d="M0 186 C68 172 105 139 150 150 S245 114 300 126 S393 78 450 92 S534 54 600 66 S688 42 750 48 S835 29 900 37" fill="none" stroke="#e6ad54" stroke-width="3" vector-effect="non-scaling-stroke" />
                  <path d="M0 151 C70 161 106 174 150 164 S239 177 300 170 S395 179 450 174 S540 185 600 178 S688 190 750 184 S845 194 900 187" fill="none" stroke="#708a8d" stroke-width="2" stroke-dasharray="5 6" vector-effect="non-scaling-stroke" />
                </svg>
                <div class="x-axis"><span>Aug 24</span><span>Aug 25</span><span>Aug 26</span><span>Aug 27</span><span>Aug 28</span><span>Aug 29</span><span>Today</span></div>
              </div>
            </div>
          </section>

          <div class="overview-grid">
            <section class="panel repo-panel">
              <div class="panel-heading"><div><h2>Hit rate by repository</h2><p>Where your cache is working hardest</p></div><button class="text-button" @click="section = 'namespaces'">View all <span>→</span></button></div>
              <div class="repo-list">
                <div v-for="row in repoRows.slice(0, 4)" :key="row.repo + row.ref" class="repo-row">
                  <span class="repo-icon" :style="{ '--repo-color': row.color }"><Icon name="box" /></span>
                  <span class="repo-name"><b>{{ row.repo }}</b><small>{{ row.ref }}</small></span>
                  <span class="mini-bar"><i :style="{ width: row.hits }"></i></span><strong>{{ row.hits }}</strong>
                </div>
              </div>
            </section>
            <section class="panel misses-panel">
              <div class="panel-heading"><div><h2>Top missing actions</h2><p>Highest rebuild cost this period</p></div><button class="text-button" @click="section = 'actions'">View all <span>→</span></button></div>
              <button v-for="(item, index) in misses.slice(0, 4)" :key="item.action" class="miss-row" @click="section = 'actions'">
                <span class="rank">{{ index + 1 }}</span><span><b>{{ item.action }}</b><small>{{ item.context }}</small></span><span class="miss-cost"><b>{{ item.cost }}</b><small>{{ item.misses.toLocaleString() }} misses</small></span><Icon name="chevron" />
              </button>
            </section>
          </div>
        </template>

        <template v-else-if="section === 'namespaces'">
          <div class="page-heading compact"><div><p class="eyebrow">Cache inventory</p><h1>Namespaces</h1><p>Usage and performance across isolated cache namespaces.</p></div><button class="primary-button"><Icon name="plus" />New namespace</button></div>
          <section class="panel table-panel">
            <div class="table-tools"><label class="search"><Icon name="search" /><input v-model="search" placeholder="Search namespaces…" /></label><span>{{ filteredNamespaces.length }} namespaces</span></div>
            <div class="data-table namespace-table"><div class="table-header"><span>Namespace</span><span>Actions</span><span>Storage</span><span>Hit rate</span><span>Last request</span><span></span></div>
              <button v-for="item in filteredNamespaces" :key="item.name" class="table-row"><span class="namespace-name"><i><Icon name="layers" /></i><b>{{ item.name }}</b></span><span>{{ item.actions }}</span><span>{{ item.size }}</span><span><b>{{ item.hits }}</b><small :class="item.trend > 0 ? 'up' : 'down'">{{ item.trend > 0 ? '↑' : '↓' }} {{ Math.abs(item.trend) }}%</small></span><span>{{ item.seen }}</span><span><Icon name="more" /></span></button>
            </div>
          </section>
          <section class="panel table-panel repo-ref-panel"><div class="panel-heading"><div><h2>Repositories and refs</h2><p>Request volume and compute saved by source</p></div><div class="filters"><select v-model="repo"><option>All repositories</option><option>acme/web</option><option>acme/api</option><option>acme/mobile</option><option>infra/tooling</option></select><select v-model="refFilter"><option>All refs</option><option>main</option><option>pull/*</option><option>release/*</option></select></div></div>
            <div class="data-table repo-table"><div class="table-header"><span>Repository</span><span>Ref</span><span>Hit rate</span><span>Requests</span><span>Compute saved</span></div><div v-for="item in filteredRepoRows" :key="item.repo + item.ref" class="table-row"><span><b>{{ item.repo }}</b></span><span><code>{{ item.ref }}</code></span><span class="rate-cell"><span class="mini-bar"><i :style="{ width: item.hits }"></i></span><b>{{ item.hits }}</b></span><span>{{ item.requests }}</span><span>{{ item.saved }}</span></div></div>
          </section>
        </template>

        <template v-else-if="section === 'actions'">
          <div class="page-heading compact"><div><p class="eyebrow">Optimization queue</p><h1>Missing actions</h1><p>Prioritize the misses that cost your team the most build time.</p></div><label class="select-wrap"><Icon name="clock" /><select v-model="range"><option>24 hours</option><option>7 days</option><option>30 days</option></select><Icon name="chevron" /></label></div>
          <div class="action-summary"><span><b>8,468</b><small>Total misses</small></span><span><b>29m 19s</b><small>Rebuild time</small></span><span><b>24</b><small>Recurring actions</small></span></div>
          <section class="panel table-panel"><div class="panel-heading"><div><h2>Actions by rebuild cost</h2><p>Repeated misses ordered by total compute impact</p></div><button class="secondary-button">Export CSV</button></div>
            <div class="data-table actions-table"><div class="table-header"><span>Action</span><span>Repository / ref</span><span>Likely cause</span><span>Misses</span><span>Total cost</span><span></span></div><button v-for="item in misses" :key="item.action" class="table-row"><span class="action-name"><i><Icon name="box" /></i><b>{{ item.action }}</b></span><span>{{ item.context }}</span><span><em>{{ item.cause }}</em></span><span>{{ item.misses.toLocaleString() }}</span><span><b>{{ item.cost }}</b></span><span><Icon name="chevron" /></span></button></div>
          </section>
        </template>

        <template v-else>
          <div class="page-heading compact"><div><p class="eyebrow">Security</p><h1>Tokens & grants</h1><p>Control who can read from and write to cache namespaces.</p></div><button class="primary-button" @click="showCreateToken = true"><Icon name="plus" />Create token</button></div>
          <div class="notice"><span><Icon name="shield" /></span><div><b>OIDC is configured for GitHub Actions</b><p>Trusted workflows receive short-lived access without storing secrets.</p></div><button @click="inspectGrant('github-actions')">Review grant</button></div>
          <section class="panel table-panel"><div class="panel-heading"><div><h2>Access grants</h2><p>{{ grants.length }} identities can access this server</p></div><button class="secondary-button"><Icon name="plus" />Add OIDC grant</button></div>
            <div class="data-table grants-table"><div class="table-header"><span>Name</span><span>Type</span><span>Namespace</span><span>Permission</span><span>Last used</span><span></span></div><button v-for="grant in grants" :key="grant.name" class="table-row" @click="inspectGrant(grant.name)"><span class="grant-name"><i :class="grant.kind.toLowerCase()"><Icon :name="grant.kind === 'OIDC' ? 'shield' : 'key'" /></i><span><b>{{ grant.name }}</b><small>{{ grant.identity }}</small></span></span><span><em>{{ grant.kind }}</em></span><span><code>{{ grant.scope }}</code></span><span>{{ grant.access }}</span><span><i v-if="grant.status === 'expiring'" class="warning-dot"></i>{{ grant.used }}</span><span><Icon name="more" /></span></button></div>
          </section>
        </template>
      </div>
    </main>

    <Transition name="toast"><div v-if="toast" class="toast"><span>✓</span>{{ toast }}</div></Transition>

    <div v-if="showCreateToken" class="modal-backdrop" @click.self="closeTokenModal">
      <section class="modal" role="dialog" aria-modal="true" aria-labelledby="token-title">
        <button class="modal-close" aria-label="Close" @click="closeTokenModal"><Icon name="x" /></button>
        <template v-if="!revealedToken">
          <span class="modal-icon"><Icon name="key" /></span><h2 id="token-title">Create access token</h2><p>Use a scoped token for machines that cannot authenticate with OIDC.</p>
          <label>Token name<input v-model="tokenName" autofocus placeholder="e.g. staging-runner" /></label>
          <label>Permission<select v-model="tokenScope"><option>Read + write</option><option>Read only</option></select></label>
          <label>Namespace pattern<input v-model="tokenNamespace" /></label>
          <div class="modal-actions"><button class="secondary-button" @click="closeTokenModal">Cancel</button><button class="primary-button" :disabled="!tokenName.trim()" @click="createToken">Create token</button></div>
        </template>
        <template v-else>
          <span class="modal-icon success"><Icon name="lock" /></span><h2>Token created</h2><p>Copy this token now. For your security, it won’t be shown again.</p>
          <button class="token-value" @click="copyToken"><code>{{ revealedToken }}</code><Icon name="copy" /></button>
          <div class="modal-actions single"><button class="primary-button" @click="closeTokenModal">Done</button></div>
        </template>
      </section>
    </div>

    <div v-if="showGrant" class="modal-backdrop" @click.self="showGrant = false">
      <section class="modal grant-modal" role="dialog" aria-modal="true"><button class="modal-close" aria-label="Close" @click="showGrant = false"><Icon name="x" /></button><span class="modal-icon"><Icon :name="selectedGrantData?.kind === 'OIDC' ? 'shield' : 'key'" /></span><h2>{{ selectedGrantData?.name }}</h2><p>{{ selectedGrantData?.identity }}</p><dl><div><dt>Type</dt><dd>{{ selectedGrantData?.kind }}</dd></div><div><dt>Namespace</dt><dd><code>{{ selectedGrantData?.scope }}</code></dd></div><div><dt>Permission</dt><dd>{{ selectedGrantData?.access }}</dd></div><div><dt>Last used</dt><dd>{{ selectedGrantData?.used }}</dd></div></dl><div class="modal-actions"><button class="danger-button" @click="revokeGrant">Revoke grant</button><button class="primary-button" @click="showGrant = false">Done</button></div></section>
    </div>
  </div>
</template>

<script lang="ts">
import { defineComponent, h } from "vue";

const paths: Record<IconName, string[]> = {
  activity: ["M3 12h4l2.5-7 5 14 2.5-7H21"], box: ["m4 7 8-4 8 4-8 4-8-4Z", "m4 7 8 4 8-4", "v10l-8 4-8-4V7", "M12 11v10"],
  chevron: ["m9 18 6-6-6-6"], clock: ["M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20Z", "M12 6v6l4 2"], copy: ["M8 8h12v12H8z", "M16 8V4H4v12h4"], database: ["M4 6c0 2 3.6 3 8 3s8-1 8-3-3.6-3-8-3-8 1-8 3Z", "M4 6v6c0 2 3.6 3 8 3s8-1 8-3V6", "M4 12v6c0 2 3.6 3 8 3s8-1 8-3v-6"], external: ["M14 3h7v7", "m10 11 11-11", "M21 14v7H3V3h7"], key: ["M21 2 9.6 13.4", "M15 8l3 3", "M12 12l3 3", "M8.5 19.5a4 4 0 1 1-5.7-5.7 4 4 0 0 1 5.7 5.7Z"], layers: ["m12 2 9 5-9 5-9-5 9-5Z", "m3 12 9 5 9-5", "m3 17 9 5 9-5"], lock: ["M5 10h14v11H5z", "M8 10V7a4 4 0 0 1 8 0v3"], more: ["M5 12h.01M12 12h.01M19 12h.01"], plus: ["M12 5v14M5 12h14"], search: ["m21 21-4.35-4.35", "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14Z"], settings: ["M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z", "M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2.12 2.12-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1 1.56V20h-3v-.08a1.7 1.7 0 0 0-1-1.56 1.7 1.7 0 0 0-1.88.34l-.06.06-2.12-2.12.06-.06A1.7 1.7 0 0 0 7.08 15 1.7 1.7 0 0 0 5.52 14H5v-4h.52a1.7 1.7 0 0 0 1.56-1 1.7 1.7 0 0 0-.34-1.88l-.06-.06 2.12-2.12.06.06a1.7 1.7 0 0 0 1.88.34 1.7 1.7 0 0 0 1-1.56V3.5h3v.28a1.7 1.7 0 0 0 1 1.56 1.7 1.7 0 0 0 1.88-.34l.06-.06 2.12 2.12-.06.06A1.7 1.7 0 0 0 19.4 9 1.7 1.7 0 0 0 21 10h.5v4H21a1.7 1.7 0 0 0-1.6 1Z"], shield: ["M12 22s8-3.5 8-10V5l-8-3-8 3v7c0 6.5 8 10 8 10Z", "m9 12 2 2 4-4"], trend: ["m3 17 6-6 4 4 8-8", "M15 7h6v6"], x: ["M18 6 6 18M6 6l12 12"],
};

export default defineComponent({
  components: {
    Icon: defineComponent({
      props: { name: { type: String, required: true } },
      setup(props) {
        return () => h("svg", { viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", "stroke-width": 1.8, "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true" }, paths[props.name as IconName].map((d) => h("path", { d })));
      },
    }),
  },
});
</script>

<style scoped>
* { box-sizing: border-box; }
button, input, select { font: inherit; }
button { color: inherit; }
.dashboard-shell { --amber:#e6ad54; --amber-deep:#c9892f; --teal:#7fa6a9; --bg:#110e09; --sidebar:#15110b; --panel:#1b1710; --panel-2:#211b12; --line:#34291a; --text:#f3eadb; --muted:#9f9583; background:var(--bg); color:var(--text); display:grid; font-family:"Space Grotesk",sans-serif; grid-template-columns:244px minmax(0,1fr); min-height:100vh; }
svg { display:block; height:18px; width:18px; }
.sidebar { background:linear-gradient(180deg,#19140d 0%,#120f0a 100%); border-right:1px solid var(--line); display:flex; flex-direction:column; height:100vh; padding:22px 14px 16px; position:sticky; top:0; }
.brand { align-items:center; color:var(--text); display:flex; font-size:17px; font-weight:700; gap:10px; letter-spacing:-.02em; padding:0 8px 22px; text-decoration:none; }
.brand-mark { background:var(--amber); border-radius:4px; display:grid; gap:2px; grid-template-columns:repeat(2,6px); height:25px; padding:5px; transform:rotate(-2deg); width:26px; }
.brand-mark i { background:#24180a; border-radius:1px; display:block; }.brand-mark i:first-child{grid-column:1/3;height:5px}.brand-mark i:nth-child(2),.brand-mark i:nth-child(3){height:6px}
.workspace-picker { align-items:center; background:var(--panel-2); border:1px solid var(--line); border-radius:9px; display:grid; gap:9px; grid-template-columns:32px 1fr 16px; margin-bottom:24px; padding:9px; }
.workspace-picker > svg { height:14px; transform:rotate(90deg); width:14px; }.workspace-avatar,.user-avatar,.avatar{align-items:center;background:#41321d;border:1px solid #5b4325;border-radius:7px;color:var(--amber);display:flex;font-family:"JetBrains Mono",monospace;font-size:11px;font-weight:700;height:32px;justify-content:center;width:32px}.workspace-picker b,.workspace-picker small,.account b,.account small{display:block}.workspace-picker b{font-size:12px}.workspace-picker small,.account small{color:var(--muted);font-size:10px;margin-top:2px}
.nav-label { color:#6f6658; font-family:"JetBrains Mono",monospace; font-size:9px; font-weight:600; letter-spacing:.12em; margin:0 10px 8px; text-transform:uppercase; }.nav-label.access-label{margin-top:24px}
nav button { align-items:center; background:transparent; border:0; border-radius:7px; color:var(--muted); cursor:pointer; display:flex; font-size:13px; font-weight:600; gap:11px; margin:2px 0; padding:9px 10px; text-align:left; transition:.15s ease; width:100%; } nav button:hover{background:#241c11;color:var(--text)}nav button.active{background:#2a2012;color:var(--amber)}nav button.active svg{stroke-width:2.2}.count{background:#38281a;border-radius:10px;color:var(--amber);font:600 9px "JetBrains Mono",monospace;margin-left:auto;padding:2px 6px}
.server-card{background:#1b1811;border:1px solid var(--line);border-radius:9px;margin-top:auto;padding:12px}.server-card div{align-items:center;display:flex;font-size:11px;gap:7px}.status-dot,.live i{background:#75b87d;border-radius:50%;box-shadow:0 0 0 3px rgb(117 184 125 / .1);height:6px;width:6px}.server-card p{color:var(--muted);font:9px "JetBrains Mono",monospace;margin:6px 0 10px 13px}.server-card a{align-items:center;color:var(--amber);display:flex;font-size:10px;gap:5px;text-decoration:none}.server-card a svg{height:11px;width:11px}.account{align-items:center;background:transparent;border:0;display:grid;gap:9px;grid-template-columns:32px 1fr 18px;margin-top:12px;padding:7px;text-align:left;width:100%}.account b{font-size:11px}.account>svg{color:var(--muted)}
main{min-width:0}.topbar{align-items:center;border-bottom:1px solid var(--line);display:flex;height:65px;justify-content:space-between;padding:0 34px}.breadcrumb{align-items:center;display:flex;font-size:12px;gap:9px}.breadcrumb span,.breadcrumb i{color:var(--muted);font-style:normal}.top-actions{align-items:center;display:flex;gap:11px}.live{align-items:center;color:#88be8e;display:flex;font:600 10px "JetBrains Mono",monospace;gap:7px;margin-right:8px}.icon-button{align-items:center;background:transparent;border:1px solid var(--line);border-radius:7px;display:flex;height:34px;justify-content:center;width:34px}.icon-button svg{color:var(--muted);height:15px;width:15px}.avatar{border-radius:50%;height:34px;width:34px}
.content{margin:0 auto;max-width:1450px;padding:42px 46px 64px}.page-heading{align-items:flex-end;display:flex;justify-content:space-between;margin-bottom:28px}.page-heading.compact{align-items:center}.eyebrow{color:var(--amber);font:600 10px "JetBrains Mono",monospace;letter-spacing:.1em;margin:0 0 8px;text-transform:uppercase}.page-heading h1{font-size:30px;letter-spacing:-.035em;line-height:1.1;margin:0}.page-heading p:not(.eyebrow){color:var(--muted);font-size:13px;margin:8px 0 0}.select-wrap{align-items:center;background:var(--panel);border:1px solid var(--line);border-radius:7px;display:flex;height:36px;padding:0 10px}.select-wrap>svg{color:var(--muted);height:14px;width:14px}.select-wrap>svg:last-child{transform:rotate(90deg)}select{appearance:none;background:transparent;border:0;color:var(--text);cursor:pointer;font-size:11px;outline:0;padding:0 24px 0 8px}select option{background:var(--panel)}
.metrics{display:grid;gap:12px;grid-template-columns:repeat(4,1fr);margin-bottom:14px}.metrics article{background:linear-gradient(145deg,#211a10,var(--panel) 60%);border:1px solid var(--line);border-radius:10px;min-height:136px;padding:17px 18px;position:relative}.metric-icon{align-items:center;border-radius:7px;display:flex;height:30px;justify-content:center;position:absolute;right:16px;top:16px;width:30px}.metric-icon svg{height:15px;width:15px}.metric-icon.amber{background:rgb(230 173 84 / .13);color:var(--amber)}.metric-icon.teal{background:rgb(127 166 169 / .13);color:var(--teal)}.metric-icon.violet{background:rgb(151 131 183 / .13);color:#a78fc8}.metric-icon.green{background:rgb(117 184 125 / .13);color:#88bd8f}.metrics p{color:var(--muted);font-size:11px;margin:0 0 12px}.metrics strong{display:block;font:600 25px "JetBrains Mono",monospace;letter-spacing:-.05em}.metrics small{color:var(--muted);display:block;font:9px "JetBrains Mono",monospace;margin-top:8px}.metrics small.up{color:#77b47d}.metrics small i{color:var(--muted);font-style:normal}.storage-bar{background:#30271a;border-radius:4px;bottom:14px;height:3px;left:18px;position:absolute;right:18px}.storage-bar i{background:var(--amber);border-radius:4px;display:block;height:100%;width:42%}
.panel{background:linear-gradient(155deg,#1f1911,var(--panel));border:1px solid var(--line);border-radius:10px}.panel-heading{align-items:center;border-bottom:1px solid var(--line);display:flex;justify-content:space-between;padding:16px 18px}.panel-heading h2{font-size:14px;letter-spacing:-.01em;margin:0}.panel-heading p{color:var(--muted);font-size:10px;margin:4px 0 0}.legend{display:flex;font:9px "JetBrains Mono",monospace;gap:16px}.legend span{align-items:center;display:flex;gap:6px}.legend i{border-radius:50%;height:6px;width:6px}.hit-dot{background:var(--amber)}.miss-dot{background:var(--teal)}
.chart-panel{margin-bottom:14px}.chart{display:grid;grid-template-columns:38px 1fr;height:250px;padding:18px 20px 12px 10px}.y-axis{color:#6f675b;display:flex;flex-direction:column;font:8px "JetBrains Mono",monospace;justify-content:space-between;padding-bottom:25px}.plot{display:flex;flex-direction:column;position:relative}.gridline{border-top:1px dashed #30271c;height:25%;left:0;position:absolute;right:0}.gridline:nth-child(1){top:0}.gridline:nth-child(2){top:25%}.gridline:nth-child(3){top:50%}.gridline:nth-child(4){top:75%}.gridline:nth-child(5){top:100%}.plot svg{height:calc(100% - 23px);position:relative;width:100%;z-index:1}.x-axis{color:#6f675b;display:flex;font:8px "JetBrains Mono",monospace;justify-content:space-between;margin-top:9px}
.overview-grid{display:grid;gap:14px;grid-template-columns:1.08fr .92fr}.text-button{background:transparent;border:0;color:var(--amber);cursor:pointer;font-size:10px}.text-button span{margin-left:4px}.repo-list{padding:6px 18px}.repo-row{align-items:center;border-bottom:1px solid rgb(52 41 26 / .65);display:grid;gap:10px;grid-template-columns:32px 1fr minmax(70px,120px) 45px;padding:11px 0}.repo-row:last-child{border-bottom:0}.repo-icon{align-items:center;background:color-mix(in srgb,var(--repo-color) 12%,transparent);border-radius:6px;color:var(--repo-color);display:flex;height:28px;justify-content:center;width:28px}.repo-icon svg{height:13px;width:13px}.repo-name b,.repo-name small{display:block}.repo-name b{font:600 10px "JetBrains Mono",monospace}.repo-name small{color:var(--muted);font:8px "JetBrains Mono",monospace;margin-top:4px}.mini-bar{background:#2f271b;border-radius:4px;height:4px;overflow:hidden}.mini-bar i{background:linear-gradient(90deg,var(--amber-deep),var(--amber));border-radius:4px;display:block;height:100%}.repo-row>strong{font:600 10px "JetBrains Mono",monospace;text-align:right}.misses-panel{overflow:hidden}.miss-row{align-items:center;background:transparent;border:0;border-bottom:1px solid rgb(52 41 26 / .65);cursor:pointer;display:grid;gap:10px;grid-template-columns:22px 1fr 68px 12px;padding:10px 17px;text-align:left;width:100%}.miss-row:hover{background:#241d13}.miss-row:last-child{border-bottom:0}.rank{align-items:center;background:#2b2114;border-radius:4px;color:var(--amber);display:flex;font:600 9px "JetBrains Mono",monospace;height:20px;justify-content:center;width:20px}.miss-row b,.miss-row small{display:block}.miss-row b{font-size:10px}.miss-row small{color:var(--muted);font:8px "JetBrains Mono",monospace;margin-top:3px}.miss-cost{text-align:right}.miss-cost b{color:var(--amber);font-family:"JetBrains Mono",monospace}.miss-row>svg{color:#675e50;height:12px;transform:rotate(0);width:12px}
.primary-button,.secondary-button,.danger-button{align-items:center;border-radius:7px;cursor:pointer;display:inline-flex;font-size:11px;font-weight:700;gap:7px;justify-content:center;padding:9px 13px}.primary-button{background:linear-gradient(135deg,#f0bc69,var(--amber-deep));border:0;color:#211506;box-shadow:0 7px 20px -10px var(--amber)}.primary-button:disabled{cursor:not-allowed;filter:grayscale(1);opacity:.45}.primary-button svg,.secondary-button svg{height:14px;width:14px}.secondary-button{background:#241d13;border:1px solid var(--line);color:var(--text)}.danger-button{background:transparent;border:1px solid #754237;color:#de8879}
.table-panel{overflow:hidden;margin-bottom:14px}.table-tools{align-items:center;border-bottom:1px solid var(--line);display:flex;justify-content:space-between;padding:13px 16px}.table-tools>span{color:var(--muted);font:9px "JetBrains Mono",monospace}.search{align-items:center;background:#15110c;border:1px solid var(--line);border-radius:6px;display:flex;padding:0 10px;width:280px}.search svg{color:var(--muted);height:13px;width:13px}.search input{background:transparent;border:0;color:var(--text);font-size:10px;height:32px;outline:0;padding:0 8px;width:100%}.data-table{font-size:11px}.table-header,.table-row{align-items:center;display:grid;gap:14px;padding:0 18px}.table-header{background:#18130d;color:#746b5d;font:8px "JetBrains Mono",monospace;height:32px;letter-spacing:.05em;text-transform:uppercase}.table-row{background:transparent;border:0;border-top:1px solid rgb(52 41 26 / .6);min-height:58px;text-align:left;width:100%}.data-table button.table-row{cursor:pointer}.data-table button.table-row:hover{background:#231c12}.namespace-table .table-header,.namespace-table .table-row{grid-template-columns:2fr .7fr .7fr 1fr 1fr 24px}.namespace-name,.action-name,.grant-name{align-items:center;display:flex;gap:10px}.namespace-name>i,.action-name>i{align-items:center;background:#2c2113;border-radius:6px;color:var(--amber);display:flex;height:28px;justify-content:center;width:28px}.namespace-name svg,.action-name svg{height:13px;width:13px}.table-row small{font:8px "JetBrains Mono",monospace;margin-left:7px}.down{color:#c77065}.up{color:#77b47d}.repo-ref-panel{margin-top:18px}.filters{display:flex;gap:8px}.filters select{background:#19140d;border:1px solid var(--line);border-radius:6px;font-size:10px;padding:7px 28px 7px 9px}.repo-table .table-header,.repo-table .table-row{grid-template-columns:1.3fr .8fr 1.5fr .8fr .8fr}.repo-table code,.grants-table code{background:#2a2115;border:1px solid #3b2e1d;border-radius:4px;color:#cfb98f;font:9px "JetBrains Mono",monospace;padding:3px 6px}.rate-cell{align-items:center;display:grid;gap:10px;grid-template-columns:1fr 40px}.rate-cell b{font:9px "JetBrains Mono",monospace}
.action-summary{background:linear-gradient(120deg,#281f12,#1a1610);border:1px solid var(--line);border-radius:10px;display:grid;grid-template-columns:repeat(3,1fr);margin-bottom:14px;padding:16px 0}.action-summary span{border-right:1px solid var(--line);padding:0 22px}.action-summary span:last-child{border:0}.action-summary b,.action-summary small{display:block}.action-summary b{font:600 20px "JetBrains Mono",monospace}.action-summary small{color:var(--muted);font-size:9px;margin-top:4px}.actions-table .table-header,.actions-table .table-row{grid-template-columns:1.45fr 1.25fr 1fr .55fr .65fr 18px}.actions-table em,.grants-table em{background:#2e271b;border:1px solid #403624;border-radius:10px;color:#b8aa92;font:normal 8px "JetBrains Mono",monospace;padding:4px 7px}.actions-table .table-row>span:last-child svg{height:12px;width:12px}
.notice{align-items:center;background:linear-gradient(90deg,rgb(127 166 169 / .1),transparent);border:1px solid #334348;border-radius:10px;display:grid;gap:13px;grid-template-columns:34px 1fr auto;margin-bottom:14px;padding:14px 16px}.notice>span{align-items:center;background:rgb(127 166 169 / .12);border-radius:7px;color:var(--teal);display:flex;height:34px;justify-content:center;width:34px}.notice b{font-size:11px}.notice p{color:var(--muted);font-size:10px;margin:4px 0 0}.notice button{background:transparent;border:0;color:var(--teal);cursor:pointer;font-size:10px;font-weight:600}.grants-table .table-header,.grants-table .table-row{grid-template-columns:1.45fr .55fr .75fr .85fr .75fr 18px}.grant-name>i{align-items:center;border-radius:6px;display:flex;height:29px;justify-content:center;width:29px}.grant-name>i.oidc{background:rgb(127 166 169 / .12);color:var(--teal)}.grant-name>i.token{background:rgb(230 173 84 / .12);color:var(--amber)}.grant-name svg{height:14px;width:14px}.grant-name b,.grant-name small{display:block}.grant-name small{color:var(--muted);font-size:8px;margin:3px 0 0}.warning-dot{background:#d78642;border-radius:50%;display:inline-block;height:5px;margin-right:6px;width:5px}
.modal-backdrop{align-items:center;background:rgb(5 4 2 / .76);backdrop-filter:blur(5px);display:flex;inset:0;justify-content:center;padding:20px;position:fixed;z-index:100}.modal{background:linear-gradient(155deg,#241c12,#17130d);border:1px solid #49351e;border-radius:13px;box-shadow:0 24px 80px #000;max-width:440px;padding:28px;position:relative;width:100%}.modal-close{align-items:center;background:transparent;border:0;color:var(--muted);cursor:pointer;display:flex;position:absolute;right:16px;top:16px}.modal-close svg{height:16px;width:16px}.modal-icon{align-items:center;background:rgb(230 173 84 / .12);border-radius:8px;color:var(--amber);display:flex;height:36px;justify-content:center;margin-bottom:16px;width:36px}.modal-icon.success{background:rgb(117 184 125 / .12);color:#88bd8f}.modal h2{font-size:20px;margin:0}.modal>p{color:var(--muted);font-size:11px;line-height:1.55;margin:7px 0 22px}.modal label{color:#bfb4a0;display:block;font-size:10px;font-weight:600;margin-top:14px}.modal label input,.modal label select{background:#110e09;border:1px solid var(--line);border-radius:6px;color:var(--text);display:block;font-size:11px;height:38px;margin-top:6px;outline:0;padding:0 10px;width:100%}.modal label input:focus{border-color:var(--amber)}.modal-actions{display:flex;gap:8px;justify-content:flex-end;margin-top:24px}.modal-actions.single .primary-button{width:100%}.token-value{align-items:center;background:#100d09;border:1px dashed #705126;border-radius:7px;color:var(--amber);cursor:pointer;display:flex;gap:10px;justify-content:space-between;padding:12px;width:100%}.token-value code{font-size:9px;overflow:hidden;text-overflow:ellipsis}.token-value svg{flex:none}.grant-modal dl{border:1px solid var(--line);border-radius:7px;margin:18px 0 0}.grant-modal dl div{align-items:center;border-bottom:1px solid var(--line);display:flex;justify-content:space-between;padding:10px 12px}.grant-modal dl div:last-child{border:0}.grant-modal dt{color:var(--muted);font-size:10px}.grant-modal dd{font-size:10px;margin:0}.grant-modal code{color:var(--amber)}.toast{align-items:center;background:#292117;border:1px solid #594321;border-radius:7px;bottom:22px;box-shadow:0 15px 40px #000;color:var(--text);display:flex;font-size:11px;gap:9px;padding:11px 15px;position:fixed;right:22px;z-index:200}.toast span{color:#78b87e}.toast-enter-active,.toast-leave-active{transition:.2s}.toast-enter-from,.toast-leave-to{opacity:0;transform:translateY(8px)}
@media(max-width:1050px){.metrics{grid-template-columns:repeat(2,1fr)}.overview-grid{grid-template-columns:1fr}.content{padding:32px 28px}.actions-table .table-header,.actions-table .table-row{grid-template-columns:1.4fr 1fr .8fr .5fr .6fr 14px}.actions-table .table-row>span:nth-child(3),.actions-table .table-header>span:nth-child(3){display:none}.grants-table .table-header,.grants-table .table-row{grid-template-columns:1.4fr .5fr .75fr .8fr 16px}.grants-table .table-row>span:nth-child(5),.grants-table .table-header>span:nth-child(5){display:none}}
@media(max-width:760px){.dashboard-shell{display:block}.sidebar{bottom:0;border-right:0;border-top:1px solid var(--line);display:block;height:62px;inset:auto 0 0;padding:7px 10px;position:fixed;z-index:50}.brand,.workspace-picker,.nav-label,.server-card,.account,nav button:nth-of-type(5){display:none}.sidebar nav{display:grid;grid-template-columns:repeat(4,1fr)}nav button,nav button.active{align-items:center;background:transparent;display:flex;flex-direction:column;font-size:8px;gap:3px;margin:0;padding:4px}.count{display:none}.topbar{padding:0 18px}.breadcrumb span,.breadcrumb i{display:none}.content{padding:28px 16px 90px}.page-heading{align-items:flex-start;gap:18px}.page-heading h1{font-size:25px}.metrics{gap:8px}.metrics article{padding:14px;min-height:125px}.metrics strong{font-size:20px}.metric-icon{display:none}.chart{height:220px}.overview-grid{grid-template-columns:1fr}.repo-row{grid-template-columns:30px 1fr 65px 40px}.table-panel{overflow-x:auto}.data-table{min-width:720px}.action-summary b{font-size:15px}.notice{grid-template-columns:32px 1fr}.notice button{grid-column:2;text-align:left}.page-heading .primary-button{white-space:nowrap}}
</style>
