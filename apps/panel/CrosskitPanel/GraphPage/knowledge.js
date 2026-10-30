// Страница карты знаний: весь граф знаний контекста силовой раскладкой `cose`.
//
// Что рисовать, решает панель (`KnowledgePicture.swift`): узлы с цветом типа и размером по
// числу связей, связи с предикатом, и что подсветить. Здесь — раскладка, приближение,
// перемещение и стиль. Данные приходят строкой JSON аргументом вызова и разбираются
// `JSON.parse`, в код не вклеиваются. Обратно уходят три сообщения: клик по узлу, клик по
// связи и «вид разложен и стоит на экране» с временем раскладки.
//
// Большая карта не должна тормозить: подписи гаснут, когда их нельзя прочесть
// (`min-zoomed-font-size`), связи рисуются прямыми без стрелок (`haystack`). Стрелка и подпись
// предиката — только у подсвеченных и выбранной связи. Имена самых связанных сущностей
// (`pinned`, выбирает панель) видны всегда: их шрифт растёт обратно приближению.
//
// Сдвиг и масштаб — общая камера холста (`camera.js`), та же, что у графа окрестности.
"use strict";

(function () {
  const post = (message) => window.webkit.messageHandlers.graph.postMessage(message);
  let cy = null;

  function style(theme) {
    return [
      { selector: "node", style: {
        "background-color": "data(color)", "width": "data(size)", "height": "data(size)",
        "label": "data(label)", "font-family": "-apple-system, BlinkMacSystemFont, sans-serif",
        "font-size": 11, "color": theme.fg, "text-valign": "bottom", "text-halign": "center",
        "text-margin-y": 3, "min-zoomed-font-size": 9, "border-width": 0,
        "text-background-color": theme.solid, "text-background-opacity": 0.7,
        "text-background-padding": 1, "text-background-shape": "round-rectangle",
      } },
      { selector: "node[?pinned]", style: {
        "min-zoomed-font-size": 0, "font-weight": 600, "z-index": 5,
      } },
      // Альфу цвета линии Cytoscape не применяет — прозрачность задаётся своим свойством.
      { selector: "edge", style: {
        "curve-style": "haystack", "haystack-radius": 0, "line-color": theme.fg, "line-opacity": 0.28,
        "width": "mapData(weight, 0, 10, 0.6, 2.2)",
      } },
      { selector: "edge.told", style: {
        "curve-style": "bezier", "target-arrow-shape": "triangle", "target-arrow-color": theme.accent,
        "line-color": theme.accent, "line-opacity": 0.9, "arrow-scale": 0.8, "label": "data(label)", "font-size": 9.5,
        "color": theme.fg2, "font-family": "-apple-system, BlinkMacSystemFont, sans-serif",
        "text-background-color": theme.solid, "text-background-opacity": 0.85,
        "text-background-padding": 2, "text-background-shape": "round-rectangle",
        "text-rotation": "autorotate", "min-zoomed-font-size": 7,
      } },
      { selector: ".dim", style: { "opacity": 0.12, "text-opacity": 0 } },
      { selector: "node.hit", style: { "border-width": 3, "border-color": theme.accent, "z-index": 20 } },
      { selector: "node.near", style: { "z-index": 10 } },
      { selector: "node:selected", style: {
        "overlay-color": theme.accent, "overlay-opacity": 0.2, "overlay-padding": 5,
      } },
      { selector: "edge:selected", style: { "width": 3 } },
    ];
  }

  function create() {
    cy = cytoscape({
      container: document.getElementById("map"),
      elements: [],
      minZoom: window.crosskitCamera.MIN_ZOOM,
      maxZoom: window.crosskitCamera.MAX_ZOOM,
      // Приближает камера по жесту, разобранному панелью: собственное колесо Cytoscape до
      // страницы не доходит, а дошедшее разбиралось бы дважды.
      userZoomingEnabled: false,
      userPanningEnabled: true,
      boxSelectionEnabled: false,
      autoungrabify: true,
      selectionType: "single",
    });
    cy.on("tap", "node", (event) => post({ type: "tap", id: event.target.id() }));
    cy.on("tap", "edge", (event) => post({ type: "tapEdge", id: event.target.id() }));
    cy.on("tap", (event) => { if (event.target === cy) post({ type: "tap", id: "" }); });
    cy.on("zoom", keepPinnedReadable);
  }

  /// Имя закреплённой сущности остаётся читаемым при отдалении: шрифт растёт обратно зуму,
  /// но не больше трёх раз — на совсем мелкой карте подпись не должна закрывать соседей.
  function keepPinnedReadable() {
    const size = Math.min(33, 11 / Math.max(cy.zoom(), 0.01));
    cy.style().selector("node[?pinned]").style("font-size", Math.max(11, size)).update();
  }

  /// Начальные места — спираль по числу связей: связанные в середине, одинокие снаружи. Без
  /// случайности: одна и та же карта раскладывается одинаково, и снимок повторим.
  function seed(nodes) {
    const order = nodes.slice().sort((a, b) => b.degree - a.degree || (a.id < b.id ? -1 : 1));
    const at = {};
    order.forEach((node, index) => {
      const angle = index * 2.399963;
      const radius = 18 * Math.sqrt(index);
      at[node.id] = { x: radius * Math.cos(angle), y: radius * Math.sin(angle) };
    });
    return at;
  }

  function layout(count) {
    return {
      name: "cose", animate: false, randomize: false, fit: true, padding: 24,
      nodeDimensionsIncludeLabels: false, componentSpacing: 60, nodeOverlap: 8,
      idealEdgeLength: () => 95, edgeElasticity: () => 80, nodeRepulsion: () => 16000,
      gravity: 0.6, numIter: count > 400 ? 400 : 1000, initialTemp: 600, coolingFactor: 0.97,
    };
  }

  function highlight(found) {
    cy.batch(() => {
      cy.elements().removeClass("dim hit near told");
      if (!found.hits.length) return;
      const hits = cy.collection(found.hits.map((id) => cy.getElementById(id)).filter((one) => one.nonempty()));
      const near = cy.collection(found.near.map((id) => cy.getElementById(id)).filter((one) => one.nonempty()));
      const lit = hits.union(near);
      cy.elements().difference(lit).difference(hits.connectedEdges()).addClass("dim");
      hits.addClass("hit");
      near.addClass("near");
      hits.connectedEdges().addClass("told");
    });
    if (found.hits.length) {
      const lit = cy.collection(found.hits.concat(found.near).map((id) => cy.getElementById(id)));
      cy.fit(lit, 48);
      // Одна найденная сущность без соседей вписалась бы во весь холст одним кругом.
      if (cy.zoom() > 1.6) cy.zoom({ level: 1.6, renderedPosition: { x: cy.width() / 2, y: cy.height() / 2 } });
    } else {
      cy.fit(cy.elements(), 24);
    }
  }

  function pick(id) {
    cy.$(":selected").unselect();
    cy.edges(".picked").removeClass("picked told");
    if (!id) return;
    const element = cy.getElementById(id);
    if (element.empty()) return;
    element.select();
    if (element.isEdge()) element.addClass("picked told");
  }

  window.crosskitKnowledge = Object.assign(window.crosskitCamera.over(() => cy), {
    render(payload) {
      const data = JSON.parse(payload);
      const started = performance.now();
      const nodes = data.picture.nodes;
      if (!cy) create();
      cy.style(style(data.theme));
      cy.elements().remove();
      const at = seed(nodes);
      cy.add(nodes.map((node) => ({ group: "nodes", data: node, position: at[node.id] })));
      cy.add(data.picture.edges.map((edge) => ({ group: "edges", data: edge })));
      const run = cy.layout(layout(nodes.length));
      run.one("layoutstop", () => {
        highlight(data.found);
        keepPinnedReadable();
        const layoutMs = performance.now() - started;
        requestAnimationFrame(() => requestAnimationFrame(() =>
          post({ type: "rendered", generation: data.generation, layoutMs })));
      });
      run.run();
    },
    highlight(payload) {
      if (cy) highlight(JSON.parse(payload));
    },
    select(payload) {
      if (cy) pick(JSON.parse(payload).id);
    },
  });

  post({ type: "ready" });
})();
