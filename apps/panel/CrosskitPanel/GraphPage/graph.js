// Страница графа панели: Cytoscape.js раскладывает и рисует то, что передала панель.
//
// Что рисовать, решает панель (`GraphPicture.swift`); здесь только раскладка dagre слева
// направо и стиль из токенов, которые панель передаёт с каждым видом. Данные приходят
// строкой JSON аргументом вызова и разбираются `JSON.parse` — в код они не вклеиваются.
// Обратно уходят два сообщения: клик по узлу и «кадр с видом готов». Сдвиг и масштаб — общая
// камера холста (`camera.js`), та же, что у карты знаний.
"use strict";

(function () {
  const post = (message) => window.webkit.messageHandlers.graph.postMessage(message);
  let cy = null;

  function style(theme) {
    return [
      { selector: "node", style: {
        "label": "data(label)", "font-family": "-apple-system, BlinkMacSystemFont, sans-serif",
        "font-size": 10.5, "color": theme.fg, "text-valign": "center", "text-halign": "right",
        "text-margin-x": 5, "width": 12, "height": 12, "border-width": 0,
        // Подпись поверх чужих линий читается: под ней подложка цвета холста.
        "text-background-color": theme.solid, "text-background-opacity": 0.75,
        "text-background-padding": 1, "text-background-shape": "round-rectangle",
      } },
      { selector: "node[kind = 'conversation']", style: {
        "shape": "round-rectangle", "background-color": theme.conversation, "width": 150,
        "height": 32, "color": "#ffffff", "font-weight": 600, "font-size": 11,
        "text-halign": "center", "text-margin-x": 0, "text-wrap": "ellipsis", "text-max-width": 138,
        "text-background-opacity": 0,
      } },
      { selector: "node[kind = 'file']", style: { "shape": "round-rectangle", "background-color": theme.file } },
      { selector: "node[state = 'missing']", style: {
        "background-color": theme.solid, "border-width": 1.5, "border-style": "dashed",
        "border-color": theme.warn, "color": theme.fg2,
      } },
      { selector: "node[state = 'outside']", style: {
        "background-color": theme.solid, "border-width": 1.5, "border-style": "dashed",
        "border-color": theme.fg3, "color": theme.fg3,
      } },
      { selector: "node[kind = 'project']", style: {
        "shape": "diamond", "background-color": theme.project, "width": 16, "height": 16,
        "text-valign": "bottom", "text-halign": "center", "text-margin-x": 0, "text-margin-y": 3,
      } },
      { selector: "node[kind = 'topic']", style: {
        "shape": "round-rectangle", "width": 18, "height": 11, "background-color": theme.solid,
        "border-width": 1.5, "border-color": theme.topic,
      } },
      { selector: "node[?focus]", style: { "border-width": 2, "border-color": theme.accent, "border-style": "solid" } },
      { selector: "node:selected", style: {
        "overlay-color": theme.accent, "overlay-opacity": 0.18, "overlay-padding": 4,
      } },
      { selector: "edge", style: {
        "curve-style": "bezier", "width": 1, "line-color": theme.edge, "target-arrow-shape": "none",
        "label": "data(label)", "font-size": 9, "color": theme.fg2,
        "font-family": "-apple-system, BlinkMacSystemFont, sans-serif",
        "text-background-color": theme.solid, "text-background-opacity": 0.85,
        "text-background-padding": 2, "text-background-shape": "round-rectangle",
      } },
      { selector: "edge[?named]", style: { "width": 1.6, "line-color": theme.accent, "color": theme.fg } },
      { selector: "edge[kind = 'touched'][!changed]", style: { "line-style": "dotted" } },
      { selector: "edge.quiet", style: { "text-opacity": 0, "text-background-opacity": 0 } },
      { selector: "edge.quiet.near", style: { "text-opacity": 1, "text-background-opacity": 0.85 } },
      { selector: "edge[kind = 'in-project'], edge[kind = 'about']", style: {
        "line-style": "dashed", "line-dash-pattern": [3, 3],
      } },
    ];
  }

  function create() {
    cy = cytoscape({
      container: document.getElementById("graph"),
      elements: [],
      minZoom: window.crosskitCamera.MIN_ZOOM,
      maxZoom: window.crosskitCamera.MAX_ZOOM,
      // Колесо и щипок разбирает окно панели и передаёт камере.
      userZoomingEnabled: false,
      userPanningEnabled: true,
      boxSelectionEnabled: false,
      autoungrabify: true,
      selectionType: "single",
    });
    cy.on("tap", "node", (event) => post({ type: "tap", id: event.target.id() }));
  }

  /// Вид вписывается по ширине — все шаги видны сразу, — но не мельче, чем читается; выше окна
  /// он листается, и сначала виден фокус.
  function place(focusId) {
    const box = cy.elements().boundingBox();
    const width = cy.width() - 24;
    const zoom = Math.max(0.6, Math.min(1, width / Math.max(box.w, 1)));
    cy.zoom(zoom);
    const height = cy.height();
    const x = 12 - box.x1 * zoom + Math.max(0, (width - box.w * zoom) / 2);
    if (box.h * zoom <= height - 24) {
      cy.pan({ x, y: (height - box.h * zoom) / 2 - box.y1 * zoom });
      return;
    }
    const focus = cy.getElementById(focusId);
    const y = focus.empty() ? box.y1 : focus.position("y");
    const top = Math.min(12 - box.y1 * zoom, Math.max(height - 12 - box.y2 * zoom, height / 2 - y * zoom));
    cy.pan({ x, y: top });
  }

  function select(id) {
    cy.$(":selected").unselect();
    cy.edges().removeClass("near");
    if (!id) return;
    const node = cy.getElementById(id);
    if (node.empty()) return;
    node.select();
    // У фокуса большой беседы — десятки линий; их подписи — снова стена, и они остаются в карточке.
    const edges = node.connectedEdges();
    if (edges.length <= 12) edges.addClass("near");
    // Выбранный с клавиатуры узел должен быть виден — и по горизонтали: холст теперь сдвигают
    // и приближают.
    const at = node.renderedPosition();
    const dx = at.x < 12 || at.x > cy.width() - 12 ? cy.width() / 2 - at.x : 0;
    const dy = at.y < 12 || at.y > cy.height() - 12 ? cy.height() / 2 - at.y : 0;
    if (dx || dy) cy.panBy({ x: dx, y: dy });
  }

  window.crosskitGraph = Object.assign(window.crosskitCamera.over(() => cy), {
    render(payload) {
      const data = JSON.parse(payload);
      if (!cy) create();
      cy.style(style(data.theme));
      cy.elements().remove();
      cy.add(data.picture.nodes.map((node) => ({ group: "nodes", data: node })));
      const quiet = data.picture.quietLabels;
      cy.add(data.picture.edges.map((edge) => ({ group: "edges", data: edge, classes: quiet ? "quiet" : "" })));
      cy.layout({
        name: "dagre", rankDir: "LR", nodeSep: quiet ? 8 : 16, rankSep: quiet ? 90 : 150, edgeSep: 8,
        nodeDimensionsIncludeLabels: true, fit: false, animate: false,
      }).run();
      const focus = data.picture.nodes.find((node) => node.focus);
      place(focus ? focus.id : null);
      select(data.selected);
      // Два кадра: первый — отрисовка Cytoscape, второй — уже на экране.
      requestAnimationFrame(() => requestAnimationFrame(() =>
        post({ type: "rendered", generation: data.generation })));
    },
    select(payload) {
      if (cy) select(JSON.parse(payload).id);
    },
  });

  post({ type: "ready" });
})();
