// Камера холста: сдвиг и масштаб — одни на граф окрестности (`graph.js`) и карту знаний
// (`knowledge.js`).
//
// Клавиши и жесты разбирает окно панели (`PanelWindow`, `CanvasKey`, `GraphPage.gesture`): до
// страницы колесо не доходит, а щипок Cytoscape не ловит. Сюда приходят готовые вызовы:
// `panBy` и `zoomBy` от жестов — сразу, без анимации, жест сам плавный; `step` и `zoom` от
// клавиш — короткой анимацией. Перетаскивание фона двигает холст силами Cytoscape.
"use strict";

window.crosskitCamera = (function () {
  const MIN_ZOOM = 0.05;
  const MAX_ZOOM = 4;
  const GLIDE_MS = 140;
  const ZOOM_STEP = 1.3;

  function clamp(level) {
    return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, level));
  }

  /// Камера холста, который вернёт `holder()`: холст создаётся с первым видом, а камера — с
  /// загрузкой страницы.
  function over(holder) {
    // Куда идёт анимация клавиши. Удержание повторяет нажатие быстрее, чем анимация кончается:
    // следующий шаг считается от цели прежнего, а не от середины пути, — иначе шаги укорачивались бы.
    let heading = null;

    function settle(cy) {
      cy.stop(true, false);
      heading = null;
    }

    function glide(cy, view) {
      cy.stop(true, false);
      heading = view;
      cy.animate({ zoom: view.zoom, pan: view.pan }, {
        duration: GLIDE_MS, easing: "ease-out", complete: () => { heading = null; },
      });
    }

    function now(cy) {
      return heading || { zoom: cy.zoom(), pan: { x: cy.pan().x, y: cy.pan().y } };
    }

    /// Масштаб к точке холста: точка под ней остаётся на месте.
    function zoomedAbout(view, level, point) {
      const zoom = clamp(level);
      const ratio = zoom / view.zoom;
      return { zoom, pan: { x: point.x - (point.x - view.pan.x) * ratio, y: point.y - (point.y - view.pan.y) * ratio } };
    }

    function middle(cy) {
      return { x: cy.width() / 2, y: cy.height() / 2 };
    }

    /// Вызовы страницы; без холста — ничего.
    const calls = {
      /// Кнопки, + − 0 и ⌘= ⌘- ⌘0.
      zoom(cy, payload) {
        const step = JSON.parse(payload).step;
        if (step === "fit") {
          settle(cy);
          cy.animate({ fit: { eles: cy.elements(), padding: 24 } }, { duration: GLIDE_MS * 2, easing: "ease-out" });
          return;
        }
        const view = now(cy);
        glide(cy, zoomedAbout(view, view.zoom * (step === "in" ? ZOOM_STEP : 1 / ZOOM_STEP), middle(cy)));
      },
      /// Стрелки: сдвиг видимой зоны долями её размера. → показывает то, что правее, — холст
      /// уходит влево.
      step(cy, payload) {
        const move = JSON.parse(payload);
        const view = now(cy);
        glide(cy, { zoom: view.zoom, pan: { x: view.pan.x - move.dx * cy.width(), y: view.pan.y - move.dy * cy.height() } });
      },
      /// Элемент — в середину, масштаб прежний.
      center(cy, payload) {
        const node = cy.getElementById(JSON.parse(payload).id);
        if (node.empty()) return;
        const view = now(cy);
        const at = node.position();
        glide(cy, { zoom: view.zoom, pan: { x: cy.width() / 2 - at.x * view.zoom, y: cy.height() / 2 - at.y * view.zoom } });
      },
      /// Два пальца.
      panBy(cy, payload) {
        const move = JSON.parse(payload);
        settle(cy);
        cy.panBy({ x: move.dx, y: move.dy });
      },
      /// Щипок и колесо с ⌘ или ⌃ — к точке под курсором.
      zoomBy(cy, payload) {
        const step = JSON.parse(payload);
        settle(cy);
        cy.zoom({ level: clamp(cy.zoom() * step.factor), renderedPosition: { x: step.x, y: step.y } });
      },
      /// Где стоит холст: для пробы жестов, и ничего больше.
      state(cy) {
        return { zoom: cy.zoom(), x: cy.pan().x, y: cy.pan().y };
      },
    };

    const bound = {};
    Object.keys(calls).forEach((name) => {
      bound[name] = (payload) => {
        const cy = holder();
        return cy ? calls[name](cy, payload) : null;
      };
    });
    return bound;
  }

  return { MIN_ZOOM, MAX_ZOOM, over };
})();
