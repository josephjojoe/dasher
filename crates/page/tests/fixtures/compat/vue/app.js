// A Vue 3 app from a CDN build: options API, a component with a string
// template, data loaded with axios (XHR underneath).
const { createApp, ref, computed, onMounted, watch, nextTick } = Vue;

const JobCard = {
  props: { job: Object },
  emits: ["apply"],
  template: `
    <li class="job" :class="{ closed: !job.open }" :data-id="job.id">
      <h2>{{ job.title }}</h2>
      <span class="team">{{ job.team }}</span>
      <button v-if="job.open" type="button" @click="$emit('apply', job.title)">Apply</button>
      <span v-else class="closed-label">Closed</span>
    </li>`,
};

const SlotDemo = {
  template: `<small class="credit"><slot></slot></small>`,
};

createApp({
  components: { JobCard, SlotDemo },
  setup() {
    const title = ref("Careers");
    const company = ref("Example Corp");
    const jobs = ref([]);
    const loaded = ref(false);
    const team = ref("");
    const applied = ref([]);
    const openJobs = computed(() => jobs.value.filter((j) => j.open));
    const teams = computed(() => [...new Set(jobs.value.map((j) => j.team))].sort());
    const filtered = computed(() =>
      team.value ? jobs.value.filter((j) => j.team === team.value) : jobs.value,
    );
    watch(loaded, async () => {
      await nextTick();
      document.title = `${title.value} (${jobs.value.length})`;
    });
    onMounted(async () => {
      const { data } = await axios.get("api/jobs.json", { params: { page: 1 } });
      jobs.value = data.jobs;
      loaded.value = true;
    });
    function apply(name) {
      applied.value.push(name);
    }
    return { title, company, jobs, loaded, team, teams, openJobs, filtered, applied, apply };
  },
}).mount("#app");
