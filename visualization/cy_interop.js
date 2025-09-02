// Minimal Cytoscape interop for VP
// Exposes window.__vp_render_graph(nodes, edges)
console.log('[VP-CY] cy_interop.js loaded (patched)');
(function(){
  function ensureCy() {
    if (window.__vp_cy_instance) {
      try { console.debug('[VP-CY] reusing cy instance'); } catch(e) {}
      return window.__vp_cy_instance;
    }
    const container = document.getElementById('graph');
    if (!container) return null;
    // create cytoscape element
    const cy = cytoscape({
      container: container,
      elements: [],
      style: [
        { selector: 'node', style: { 'label': 'data(label)', 'text-valign': 'center', 'text-halign': 'center', 'background-color': 'data(color)', 'width': 'data(size)', 'height': 'data(size)', 'font-size': 10 } },
        { selector: 'edge', style: { 'curve-style': 'bezier', 'target-arrow-shape': 'triangle', 'target-arrow-color': 'data(color)', 'line-color': 'data(color)', 'width': 'data(width)', 'opacity': 0.95 } },
        { selector: 'edge.upstream', style: { 'line-color': '#ff7f0e', 'target-arrow-color': '#ff7f0e', 'opacity': 1 } },
        { selector: 'edge.downstream', style: { 'line-color': '#1f77b4', 'target-arrow-color': '#1f77b4', 'opacity': 1 } }
      ],
      layout: { name: 'preset' }
    });
    try { console.debug('[VP-CY] created cytoscape instance'); } catch (e) {}
    window.__vp_cy_instance = cy;
    return cy;
  }

  const getVal = (obj, keys) => {
    if (!obj) return undefined;
    for (const k of keys) {
      try { if (typeof obj.get === 'function') { const v = obj.get(k); if (typeof v !== 'undefined' && v !== null) return v; } } catch (e) {}
      try { if (Object.prototype.hasOwnProperty.call(obj, k)) return obj[k]; } catch (e) {}
    }
    return undefined;
  };

  const isImmediateUpstream = (maybeNodeInfo, srcStr, tgtStr, last_nodes) => {
    try {
      if (maybeNodeInfo && typeof maybeNodeInfo === 'object') {
        const v = (typeof maybeNodeInfo.get === 'function') ? maybeNodeInfo.get(srcStr) : maybeNodeInfo[srcStr];
        if (v) {
          let upv = undefined;
          if (typeof v.get === 'function') upv = v.get('upstream'); else if (v.upstream) upv = v.upstream;
          if (upv) {
            let un = undefined;
            if (typeof upv.get === 'function') un = upv.get('node_name') || upv.get('nodeName') || upv.get('node'); else un = upv.node_name || upv.nodeName || upv.node;
            if (typeof un !== 'undefined' && String(un) === tgtStr) return true;
          }
        }
      }
    } catch (e) {}
    if (Array.isArray(last_nodes)) {
      for (const n of last_nodes) {
        try {
          const nid = (n && typeof n.get === 'function') ? (n.get('id') || n.get('label')) : (n && (n.id || n.label));
          if (!nid) continue;
          if (String(nid) !== srcStr) continue;
          let up = undefined;
          if (n && typeof n.get === 'function') {
            const upmap = n.get('upstream');
            if (upmap) up = (typeof upmap.get === 'function') ? (upmap.get('node_name') || upmap.get('nodeName') || upmap.get('node')) : (upmap && (upmap.node_name || upmap.nodeName || upmap.node));
          } else {
            up = (n && n.upstream && (n.upstream.node_name || n.upstream.nodeName || n.upstream.node)) || undefined;
          }
          if (typeof up !== 'undefined' && String(up) === tgtStr) return true;
        } catch (e) {}
      }
    }
    return false;
  };

  window.__vp_render_graph = function(nodes, edges) {
    try {
      const cy = ensureCy();
      if (!cy) return;
      try { window.__vp_last_nodes = Array.isArray(nodes) ? nodes.slice() : []; window.__vp_last_edges = Array.isArray(edges) ? edges.slice() : []; } catch (e) {}

      const nodeList = Array.isArray(nodes) ? nodes : [];
      const edgeList = Array.isArray(edges) ? edges : [];
      const seenNodeIds = new Set();
      const seenEdgeIds = new Set();

      const elements = [];
      for (const n of nodeList) {
        const id = n && (n.id || n.label);
        if (!id) continue;
        seenNodeIds.add(String(id));
        const x = (typeof n.x === 'number') ? n.x*100 : 0;
        const y = (typeof n.y === 'number') ? -n.y*100 : 0;
        const color = n && n.type && typeof n.type === 'string' && n.type.toLowerCase() === 'rsu' ? 'rgba(200,30,30,0.95)' : 'rgba(30,100,200,0.95)';
        const size = Math.max(8, Number(n.size) || 12);
        elements.push({ group: 'nodes', data: { id: String(id), label: n.label, color: color, size: size }, position: { x: x, y: y } });
      }

      // Build edge elements preserving raw edge objects in data for later inspection
      for (const e of edgeList) {
        let src = getVal(e, ['source','from','src']) || getVal(e, ['src']);
        let tgt = getVal(e, ['target','to','dst']) || getVal(e, ['dst']);
        if (!src || !tgt) continue;
        const edgeId = getVal(e, ['id']) || (String(src) + '->' + String(tgt));
        seenEdgeIds.add(String(edgeId));
        const up = Number(getVal(e, ['up_bps','up'])) || 0;
        const down = Number(getVal(e, ['down_bps','down'])) || 0;
        // width heuristic
        let bw = 1;
        try {
          const total = Math.max(0, up) + Math.max(0, down);
          if (total <= 0) bw = 1; else bw = Math.min(20, Math.max(1, Math.round(Math.log10(total+1)*2)));
        } catch (ee) { bw = 1; }
        let color = getVal(e, ['color']) || (up > down ? 'rgba(200,30,30,0.9)' : (down > up ? 'rgba(30,100,200,0.9)' : 'rgba(120,120,120,0.6)'));
        color = String(color);
        // prefer explicit route_kind
        try { const rk = getVal(e, ['route_kind','routeKind','kind']); if (rk && String(rk).toLowerCase() === 'upstream') color = '#ff7f0e'; if (rk && String(rk).toLowerCase() === 'downstream') color = '#1f77b4'; } catch (ee) {}
        elements.push({ group: 'edges', data: { id: String(edgeId), source: String(src), target: String(tgt), width: bw, color: color, __raw: e } });
      }

      // perform update: remove missing, add new
      try { cy.elements().remove(); } catch (e) {}
      try { cy.add(elements); } catch (e) { console.error('[VP-CY] add elements failed', e); }

      // Post-process edges: apply inline style, detect upstream/downstream and add classes
      try {
        const maybeNodeInfo = window.__vp_node_info || window.__vp_last_node_info || null;
        for (const ed of cy.edges()) {
          try {
            const edgeId = ed.id();
            const src = ed.data('source');
            const tgt = ed.data('target');
            const raw = ed.data('__raw');
            const dataColor = ed.data('color') || '';
            ed.style('line-color', String(dataColor));
            ed.style('target-arrow-color', String(dataColor));
            let isImmediateUp = isImmediateUpstream(maybeNodeInfo, String(src), String(tgt), window.__vp_last_nodes);
            const rk = raw ? (getVal(raw, ['route_kind','routeKind','kind']) || undefined) : undefined;
            const isUp = (rk && String(rk).toLowerCase() === 'upstream') || isImmediateUp;
            const isDown = (rk && String(rk).toLowerCase() === 'downstream') || false;
            if (isUp) { try { console.debug('[VP-CY] marking upstream edge', edgeId); } catch (ee) {} ed.addClass('upstream'); } else ed.removeClass('upstream');
            if (isDown) { try { console.debug('[VP-CY] marking downstream edge', edgeId); } catch (ee) {} ed.addClass('downstream'); } else ed.removeClass('downstream');
            // fallback by color
            try {
              const col = String(String(dataColor).toLowerCase() || '');
              if (!ed.hasClass('upstream') && (col.includes('ff7f0e') || col.includes('255,127,14'))) { try { console.debug('[VP-CY] fallback-mark upstream by color', edgeId, col); } catch (ee) {} ed.addClass('upstream'); }
              if (!ed.hasClass('downstream') && (col.includes('1f77b4') || col.includes('31,119,180') || col.includes('30,119,180'))) { try { console.debug('[VP-CY] fallback-mark downstream by color', edgeId, col); } catch (ee) {} ed.addClass('downstream'); }
            } catch (ee) {}
            // microtask reapply
            try {
              setTimeout(() => {
                try {
                  ed.style('line-color', ed.data('color'));
                  ed.style('target-arrow-color', ed.data('color'));
                  const col = String(String(ed.data('color') || '').toLowerCase());
                  if (!ed.hasClass('upstream') && (col.includes('ff7f0e') || col.includes('255,127,14'))) { try { ed.addClass('upstream'); } catch (e) {} }
                  if (!ed.hasClass('downstream') && (col.includes('1f77b4') || col.includes('31,119,180') || col.includes('30,119,180'))) { try { ed.addClass('downstream'); } catch (e) {} }
                } catch (e) { try { console.debug('[VP-CY] reapply inline style failed', edgeId, e); } catch (ee) {} }
              }, 0);
            } catch (ee) {}
          } catch (ee) {}
        }
      } catch (ee) { console.error('[VP-CY] post-process edges failed', ee); }

      // final-pass to ensure classes match colors
      try {
        setTimeout(() => {
          try {
            cy.edges().forEach(ed => {
              try {
                const col = String(ed.data('color') || '').toLowerCase();
                if (col.includes('ff7f0e') || col.includes('255,127,14')) { if (!ed.hasClass('upstream')) { ed.addClass('upstream'); try { console.debug('[VP-CY] final-pass add upstream', ed.id()); } catch (e) {} } }
                else { if (ed.hasClass('upstream')) ed.removeClass('upstream'); }
                if (col.includes('1f77b4') || col.includes('31,119,180') || col.includes('30,119,180')) { if (!ed.hasClass('downstream')) { ed.addClass('downstream'); try { console.debug('[VP-CY] final-pass add downstream', ed.id()); } catch (e) {} } }
                else { if (ed.hasClass('downstream')) ed.removeClass('downstream'); }
              } catch (e) {}
            });
          } catch (e) { try { console.debug('[VP-CY] final-pass failed', e); } catch (ee) {} }
        }, 0);
      } catch (ee) {}

      try { cy.fit(); } catch (e) {}
    } catch (err) { console.error('[VP-CY]', err); }
  };
})();
