(function () {
  const statsEl = document.getElementById('stats');
  const detailsEl = document.getElementById('details');
  const searchEl = document.getElementById('search');
  const labelFilterEl = document.getElementById('label-filter');
  const reloadEl = document.getElementById('reload');

  const PALETTE = [
    '#5b8def', '#22c55e', '#f59e0b', '#ec4899',
    '#a855f7', '#14b8a6', '#ef4444', '#3b82f6',
    '#84cc16', '#f97316',
  ];
  function colorForLabel(label) {
    let h = 0;
    for (let i = 0; i < label.length; i++) h = (h * 31 + label.charCodeAt(i)) | 0;
    return PALETTE[Math.abs(h) % PALETTE.length];
  }

  function fmtValue(v) {
    if (v === null || v === undefined) return 'null';
    if (typeof v !== 'object') return String(v);
    // Property values come back as `{Str:"..."}`, `{Int:1}` etc.
    const key = Object.keys(v)[0];
    return key ? String(v[key]) : JSON.stringify(v);
  }

  async function fetchJson(url) {
    const r = await fetch(url);
    if (!r.ok) throw new Error(url + ' → ' + r.status);
    return r.json();
  }

  function toElements(g) {
    const nodes = g.nodes.map((n) => ({
      data: {
        id: 'n' + n.id,
        rawId: n.id,
        label: n.labels[0] || ('#' + n.id),
        labelColor: colorForLabel(n.labels[0] || ''),
        rawLabels: n.labels,
        props: n.props,
      },
    }));
    const edges = g.edges.map((e) => ({
      data: {
        id: 'e' + e.id,
        rawId: e.id,
        source: 'n' + e.src,
        target: 'n' + e.dst,
        label: e.type,
        props: e.props,
      },
    }));
    return [...nodes, ...edges];
  }

  function renderNodeDetails(node) {
    const d = node.data();
    const labels = (d.rawLabels || []).map((l) =>
      `<span class="chip" style="border-color:${colorForLabel(l)};color:${colorForLabel(l)}">${l}</span>`
    ).join('');
    const props = Object.entries(d.props || {}).map(([k, v]) =>
      `<div class="k">${k}</div><div class="v">${fmtValue(v)}</div>`
    ).join('');
    detailsEl.innerHTML = `
      <h2>node</h2>
      <div class="id">#${d.rawId}</div>
      <div>${labels || '<span class="hint">no labels</span>'}</div>
      <div class="kv" style="margin-top:0.6rem">${props || '<span class="hint">no properties</span>'}</div>
    `;
  }

  function renderEdgeDetails(edge) {
    const d = edge.data();
    const props = Object.entries(d.props || {}).map(([k, v]) =>
      `<div class="k">${k}</div><div class="v">${fmtValue(v)}</div>`
    ).join('');
    detailsEl.innerHTML = `
      <h2>edge</h2>
      <div class="id">#${d.rawId} · ${d.label}</div>
      <div class="kv" style="margin-top:0.6rem">${props || '<span class="hint">no properties</span>'}</div>
    `;
  }

  let cy = null;
  let currentLabels = new Set();

  function rebuildCy(elements) {
    if (cy) cy.destroy();
    cy = cytoscape({
      container: document.getElementById('cy'),
      elements,
      style: [
        {
          selector: 'node',
          style: {
            'background-color': 'data(labelColor)',
            'label': 'data(label)',
            'color': '#e6e8eb',
            'font-size': '11px',
            'text-valign': 'center',
            'text-halign': 'center',
            'width': 28,
            'height': 28,
            'border-width': 1,
            'border-color': '#1d2330',
          },
        },
        {
          selector: 'node.dimmed',
          style: { 'opacity': 0.15 },
        },
        {
          selector: 'edge',
          style: {
            'width': 1.5,
            'line-color': '#3a4151',
            'target-arrow-color': '#3a4151',
            'target-arrow-shape': 'triangle',
            'curve-style': 'bezier',
            'label': 'data(label)',
            'color': '#7d8595',
            'font-size': '9px',
            'text-rotation': 'autorotate',
          },
        },
        {
          selector: 'edge.dimmed',
          style: { 'opacity': 0.1, 'text-opacity': 0 },
        },
        {
          selector: ':selected',
          style: { 'border-color': '#f5d76e', 'border-width': 2 },
        },
      ],
      layout: { name: 'cose', animate: false },
    });

    cy.on('tap', 'node', (evt) => renderNodeDetails(evt.target));
    cy.on('tap', 'edge', (evt) => renderEdgeDetails(evt.target));
    cy.on('tap', (evt) => {
      if (evt.target === cy) {
        detailsEl.innerHTML = '<p class="hint">click a node or edge to inspect it</p>';
      }
    });
    window.cy = cy;
  }

  function applyFilters() {
    if (!cy) return;
    const label = labelFilterEl.value;
    const needle = (searchEl.value || '').toLowerCase().trim();

    let visibleNodes = cy.nodes();
    if (label) {
      visibleNodes = visibleNodes.filter((n) => (n.data('rawLabels') || []).includes(label));
    }
    if (needle) {
      visibleNodes = visibleNodes.filter((n) => {
        const labels = (n.data('rawLabels') || []).join(' ').toLowerCase();
        const props = JSON.stringify(n.data('props') || {}).toLowerCase();
        return labels.includes(needle) || props.includes(needle);
      });
    }
    const visibleIds = new Set(visibleNodes.map((n) => n.id()));

    cy.batch(() => {
      cy.nodes().forEach((n) => n.toggleClass('dimmed', !visibleIds.has(n.id())));
      cy.edges().forEach((e) => {
        const ok = visibleIds.has(e.source().id()) && visibleIds.has(e.target().id());
        e.toggleClass('dimmed', !ok);
      });
    });
    statsEl.textContent = `${visibleNodes.length}/${cy.nodes().length} nodes · ${cy.edges().filter((e) => !e.hasClass('dimmed')).length}/${cy.edges().length} edges`;
  }

  function populateLabelFilter(g) {
    currentLabels = new Set();
    g.nodes.forEach((n) => (n.labels || []).forEach((l) => currentLabels.add(l)));
    const prev = labelFilterEl.value;
    labelFilterEl.innerHTML = '<option value="">all labels</option>' +
      [...currentLabels].sort().map((l) => `<option value="${l}">${l}</option>`).join('');
    if ([...currentLabels].includes(prev)) labelFilterEl.value = prev;
  }

  async function load() {
    statsEl.textContent = 'loading…';
    try {
      const g = await fetchJson('/api/graph');
      populateLabelFilter(g);
      rebuildCy(toElements(g));
      applyFilters();
    } catch (err) {
      statsEl.textContent = 'load failed: ' + err.message;
    }
  }

  searchEl.addEventListener('input', applyFilters);
  labelFilterEl.addEventListener('change', applyFilters);
  reloadEl.addEventListener('click', load);

  load();
})();
