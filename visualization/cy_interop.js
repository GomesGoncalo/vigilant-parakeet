// Minimal Cytoscape interop for VP
// Exposes window.__vp_render_graph(nodes, edges)
console.log('[VP-CY] cy_interop.js loaded');
(function(){
  function ensureCy() {
    if (window.__vp_cy && window.__vp_cy_instance) return window.__vp_cy_instance;
    const container = document.getElementById('graph');
    if (!container) return null;
    // clear container
    container.innerHTML = '';
    // create cytoscape element
    const cy = cytoscape({
      container: container,
      elements: [],
          style: [
            { selector: 'node', style: { 'label': 'data(label)', 'text-valign': 'center', 'text-halign': 'center', 'background-color': 'data(color)', 'width': 'data(size)', 'height': 'data(size)', 'font-size': 10 } },
                        { selector: 'edge', style: { 'curve-style': 'bezier', 'target-arrow-shape': 'triangle', 'target-arrow-color': 'data(color)', 'line-color': 'data(color)', 'width': 'data(width)', 'opacity': 0.95 } }
          ],
      layout: { name: 'preset' },
    });
    window.__vp_cy_instance = cy;
    return cy;
  }

  window.__vp_render_graph = function(nodes, edges) {
    try {
      const cy = ensureCy();
      if (!cy) return;
      // Normalize inputs (they may be wasm Map-produced objects)
      const nodeList = Array.isArray(nodes) ? nodes : ([]);
      const edgeList = Array.isArray(edges) ? edges : ([]);
      // convert to cytoscape elements
      const elements = [];
      for (const n of nodeList) {
        const id = n.id || n.label;
        const x = (typeof n.x === 'number') ? n.x*100 : 0;
        const y = (typeof n.y === 'number') ? -n.y*100 : 0; // invert y for nicer layout
        const color = n.type && n.type.toLowerCase() === 'rsu' ? 'rgba(200,30,30,0.95)' : 'rgba(30,100,200,0.95)';
        const size = Math.max(8, Number(n.size) || 12);
        elements.push({ group: 'nodes', data: { id: id, label: n.label, color: color, size: size }, position: { x: x, y: y } });
      }
      for (const e of edgeList) {
        const id = e.id || (e.source + '->' + e.target);
        // edge width scaled from up/down bps; use max of both
        const up = Number(e.up_bps) || 0;
        const down = Number(e.down_bps) || 0;
        const bw = Math.max(1, Math.min(12, Math.round((Math.log10(Math.max(1, up)+1) + Math.log10(Math.max(1, down)+1))*1.8)));
        const color = up > down ? 'rgba(200,30,30,0.9)' : (down > up ? 'rgba(30,100,200,0.9)' : 'rgba(120,120,120,0.6)');
        elements.push({ group: 'edges', data: { id: id, source: e.source, target: e.target, width: bw, color: color } });
      }
      // update cy
      cy.elements().remove();
      cy.add(elements);
      cy.fit();
    } catch (err) { console.error('[VP-CY]', err); }
  };
})();
