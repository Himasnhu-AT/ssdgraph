(async function () {
  const statsEl = document.getElementById('stats');

  async function fetchGraph() {
    const r = await fetch('/api/graph');
    if (!r.ok) throw new Error('graph fetch failed');
    return r.json();
  }

  function toCyElements(g) {
    const nodes = g.nodes.map((n) => ({
      data: {
        id: 'n' + n.id,
        label: n.labels[0] || ('#' + n.id),
        props: n.props,
        rawLabels: n.labels,
      },
    }));
    const edges = g.edges.map((e) => ({
      data: {
        id: 'e' + e.id,
        source: 'n' + e.src,
        target: 'n' + e.dst,
        label: e.type,
        props: e.props,
      },
    }));
    return { nodes, edges };
  }

  try {
    const g = await fetchGraph();
    const elements = toCyElements(g);

    const cy = cytoscape({
      container: document.getElementById('cy'),
      elements: [...elements.nodes, ...elements.edges],
      style: [
        {
          selector: 'node',
          style: {
            'background-color': '#5b8def',
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
      ],
      layout: { name: 'cose', animate: false },
    });

    statsEl.textContent = `${g.nodes.length} nodes · ${g.edges.length} edges`;
    window.cy = cy;
  } catch (err) {
    statsEl.textContent = 'failed to load graph: ' + err.message;
  }
})();
