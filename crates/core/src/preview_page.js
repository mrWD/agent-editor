// The script the Preview relay adds to every page of a previewed app.
//
// It runs inside the app's page, which the interface cannot touch from outside,
// and does three things when the interface asks: shows the page in a light or a
// dark theme, lets the user pick an element, and lets the user draw on the page.
// It talks to the interface in messages between windows and to nobody else.
//
// Plain JavaScript without anything newer than every current browser engine
// knows, and without dependencies: it must not disturb the page it is a guest of.
(function (config) {
  'use strict';

  // Only a page shown straight in a frame has somebody to talk to. Opened in a tab
  // of its own, or as a frame inside the app, the page is left exactly as it is.
  if (window.parent === window || window.parent !== window.top) return;
  if (window.__aePreviewPage) return;
  window.__aePreviewPage = true;

  var FROM_INTERFACE = 'ae-preview';
  var FROM_PAGE = 'ae-preview-page';
  var BLUE = '#1F6FEB';
  var RED = '#D1242F';

  // ---------- talking to the interface ----------

  /** The address of the interface, known once it has spoken. Nothing but "ready" is said before that. */
  var parentOrigin = null;

  function say(type, data) {
    if (!parentOrigin) return;
    var message = { source: FROM_PAGE, type: type };
    for (var key in data || {}) message[key] = data[key];
    window.parent.postMessage(message, parentOrigin);
  }

  window.addEventListener('message', function (event) {
    if (event.source !== window.parent) return;
    // Only the windows that asked the core for this preview are listened to.
    if (config.parents.indexOf(event.origin) < 0) return;
    var message = event.data;
    if (!message || message.source !== FROM_INTERFACE) return;
    parentOrigin = event.origin;
    try {
      handle(message);
    } catch (error) {
      say('problem', { text: String((error && error.message) || error) });
    }
  });

  function handle(message) {
    switch (message.type) {
      case 'setup':
        // The interface has just met this page: it is told everything anew.
        lastTheme = '';
        lastInfo = '';
        setTheme(message.theme);
        setTool(message.tool);
        setMark(message.mark);
        setInk(message.ink || []);
        tellInfo();
        break;
      case 'theme':
        setTheme(message.theme);
        break;
      case 'tool':
        setTool(message.tool);
        break;
      case 'mark':
        setMark(message.mark);
        break;
      case 'ink':
        setInk(message.ink || []);
        break;
      case 'parent':
        pickParent();
        break;
      case 'reload':
        window.location.reload();
        break;
      case 'hello':
        // Asked after every load, so the interface knows whether this page can be talked to.
        say('here', {});
        break;
    }
  }

  // ---------- what the page is ----------

  /** Whether the page keeps itself fresh: a dev server's client that reloads or patches it on a change. */
  var hot = false;
  var HOT_ADDRESS =
    /(\/@vite\/|_next\/webpack-hmr|webpack-hmr|__webpack_hmr|turbopack|sockjs-node|livereload|browser-sync|\/__hmr|\/_hmr|\/hmr\b|\/esbuild$|\/ws$)/i;

  function noteConnection(address, protocols) {
    var words = [].concat(protocols || []).join(' ');
    if (/vite-(hmr|ping)/.test(words) || HOT_ADDRESS.test(String(address).split('?')[0])) {
      if (!hot) {
        hot = true;
        tellInfo();
      }
    }
  }

  // Dev servers talk to their pages over a WebSocket or an event stream. Seeing one
  // opened is how the page is known to reload itself; the connection itself is untouched.
  ['WebSocket', 'EventSource'].forEach(function (name) {
    var Native = window[name];
    if (typeof Native !== 'function' || typeof Proxy !== 'function') return;
    try {
      window[name] = new Proxy(Native, {
        construct: function (target, args) {
          noteConnection(args[0], name === 'WebSocket' ? args[1] : null);
          return Reflect.construct(target, args);
        },
      });
    } catch (error) {
      // An engine that will not have it wrapped: the page then counts as one that does not reload itself.
    }
  });

  var lastInfo = '';
  function tellInfo() {
    if (!parentOrigin) return;
    if (!hot && document.querySelector('script[src*="/@vite/client"]')) hot = true;
    var problem = document.querySelector('meta[name="ae-preview-problem"]');
    var info = {
      path: window.location.pathname + window.location.search + window.location.hash,
      title: document.title,
      hot: hot,
      problem: problem ? problem.getAttribute('content') : null,
    };
    var text = JSON.stringify(info);
    if (text === lastInfo) return;
    lastInfo = text;
    say('info', info);
  }
  document.addEventListener('DOMContentLoaded', tellInfo);
  window.addEventListener('load', tellInfo);
  // A page that changes its address without loading anew tells nobody; a look now and then finds out.
  setInterval(tellInfo, 1000);

  // ---------- the theme ----------
  //
  // "Dark" means what the system's dark mode means to the page: the answer to
  // `prefers-color-scheme` in its style sheets and scripts, and the scheme its
  // `color-scheme` picks. The interface asks the browser for that on the frame itself;
  // where the browser does not pass it on to the page, the page's own style sheets are
  // rewritten to the same effect. What could not be done is reported, never assumed.

  var nativeMatchMedia = window.matchMedia ? window.matchMedia.bind(window) : null;
  var SCHEME_QUERY = /\(\s*prefers-color-scheme\s*:\s*(light|dark)\s*\)/gi;
  var ALWAYS = '(min-width: 0px)';
  var NEVER = '(max-width: 0px)';

  /** The theme the interface asked for: 'light', 'dark', or null for "as the system has it". */
  var wanted = null;
  /** Whether the page's own answer to `prefers-color-scheme` has to be replaced by ours. */
  var mediaForced = false;
  /** Whether the page's own pick between `color-scheme: light dark` has to be replaced by ours. */
  var schemeForced = false;
  /** Media lists and style declarations that were rewritten, with what they said before. */
  var changedMedia = typeof WeakMap === 'function' ? new WeakMap() : null;
  var changedSchemes = typeof WeakMap === 'function' ? new WeakMap() : null;
  var changedList = [];
  /**
   * Lists handed to the page's scripts by `matchMedia`, by the question asked, so they can be told
   * when the theme changes. One list per question: a page that asks on every draw gets the same one back.
   */
  var handedOut = {};
  var pageAsked = false;

  function rewritten(query) {
    return query.replace(SCHEME_QUERY, function (whole, scheme) {
      return scheme.toLowerCase() === wanted ? ALWAYS : NEVER;
    });
  }

  function mentionsScheme(query) {
    SCHEME_QUERY.lastIndex = 0;
    return SCHEME_QUERY.test(query);
  }

  /** What the browser itself answers inside this frame, before anything of ours. */
  function nativeMedia() {
    if (!nativeMatchMedia) return null;
    return nativeMatchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }

  /** Which scheme the browser picks here for `color-scheme: light dark`, found out by trying. */
  function nativeScheme() {
    var root = document.documentElement;
    if (!root) return null;
    var colors = ['light dark', 'light', 'dark'].map(function (scheme) {
      var probe = document.createElement('span');
      probe.style.cssText = 'position:absolute;width:0;height:0;color:CanvasText;color-scheme:' + scheme;
      root.appendChild(probe);
      var color = getComputedStyle(probe).color;
      root.removeChild(probe);
      return color;
    });
    if (colors[1] === colors[2]) return null;
    return colors[0] === colors[2] ? 'dark' : 'light';
  }

  // The page's scripts ask through this. A question about the colour scheme gets a list
  // that answers for the theme being shown and says so again when the theme changes.
  if (nativeMatchMedia) {
    window.matchMedia = function (query) {
      var text = String(query);
      if (!mentionsScheme(text)) return nativeMatchMedia(text);
      pageAsked = true;
      var key = 'q:' + text;
      if (!handedOut[key]) handedOut[key] = themedList(text);
      return handedOut[key].public;
    };
  }

  function themedList(query) {
    var listeners = [];
    var inner = null;
    /** The answer the page was last given or told. A change is announced against this, whoever brought it. */
    var told = false;
    var state = { public: null, refresh: refresh };

    function attach() {
      if (inner) {
        if (inner.removeEventListener) inner.removeEventListener('change', announce);
        else inner.removeListener(announce);
      }
      inner = nativeMatchMedia(mediaForced ? rewritten(query) : query);
      if (inner.addEventListener) inner.addEventListener('change', announce);
      else inner.addListener(announce);
    }
    /** Tells the page's listeners when the answer is not the one they last heard. */
    function announce() {
      if (inner.matches === told) return;
      told = inner.matches;
      var event;
      try {
        event = new MediaQueryListEvent('change', { matches: told, media: query });
      } catch (error) {
        event = { type: 'change', matches: told, media: query };
      }
      listeners.slice().forEach(function (listener) {
        if (typeof listener === 'function') listener.call(list, event);
        else if (listener && typeof listener.handleEvent === 'function') listener.handleEvent(event);
      });
      if (typeof list.onchange === 'function') list.onchange(event);
    }
    // The browser's own list is swapped for another when the theme is forced or let go. A change
    // the old list was about to report would be lost with it, so the answer is compared here.
    function refresh() {
      attach();
      announce();
    }
    function add(listener) {
      if (listener && listeners.indexOf(listener) < 0) listeners.push(listener);
    }
    function remove(listener) {
      var at = listeners.indexOf(listener);
      if (at >= 0) listeners.splice(at, 1);
    }

    // Made from the real kind of object, so a page that checks what it was given is satisfied.
    var proto = window.MediaQueryList ? window.MediaQueryList.prototype : Object.prototype;
    var list = Object.create(proto, {
      matches: {
        get: function () {
          return inner.matches;
        },
        enumerable: true,
      },
      media: { value: query, enumerable: true },
      onchange: { value: null, writable: true, enumerable: true },
      addListener: { value: add },
      removeListener: { value: remove },
      addEventListener: {
        value: function (type, listener) {
          if (type === 'change') add(listener);
        },
      },
      removeEventListener: {
        value: function (type, listener) {
          if (type === 'change') remove(listener);
        },
      },
      dispatchEvent: {
        value: function () {
          return true;
        },
      },
    });
    state.public = list;
    attach();
    told = inner.matches;
    return state;
  }

  /** Puts back everything that was rewritten, so the next pass starts from what the page wrote. */
  function restoreStyles() {
    changedList.forEach(function (change) {
      try {
        if (change.kind === 'media') change.target.mediaText = change.before;
        else if (change.kind === 'scheme') change.target.setProperty('color-scheme', change.before, change.priority);
        else if (change.kind === 'meta') change.target.setAttribute('content', change.before);
      } catch (error) {
        // The sheet went away with a hot update; there is nothing to put back.
      }
    });
    changedList = [];
    if (typeof WeakMap === 'function') {
      changedMedia = new WeakMap();
      changedSchemes = new WeakMap();
    }
  }

  /** Whether a `color-scheme` value lets the browser choose: it names both light and dark. */
  function choosesScheme(value) {
    var words = String(value || '')
      .toLowerCase()
      .split(/\s+/);
    return words.indexOf('light') >= 0 && words.indexOf('dark') >= 0;
  }

  /**
   * Goes through every style sheet the page has and, where the browser does not show the
   * wanted theme by itself, rewrites what depends on the system's theme. Returns what was
   * found, so the interface can say what the page has and what could not be reached.
   */
  function applyStyles() {
    var found = { media: 0, schemes: 0, unreadable: 0 };

    function media(list) {
      if (!list || !changedMedia) return;
      var before = changedMedia.has(list) ? changedMedia.get(list) : list.mediaText;
      if (!mentionsScheme(before)) return;
      found.media++;
      if (!mediaForced || changedMedia.has(list)) return;
      changedMedia.set(list, before);
      changedList.push({ kind: 'media', target: list, before: before });
      list.mediaText = rewritten(before);
    }

    function declarations(style) {
      if (!style || !changedSchemes || typeof style.getPropertyValue !== 'function') return;
      var before = changedSchemes.has(style) ? changedSchemes.get(style) : style.getPropertyValue('color-scheme');
      if (!choosesScheme(before)) return;
      found.schemes++;
      if (!schemeForced || changedSchemes.has(style)) return;
      var priority = style.getPropertyPriority('color-scheme');
      changedSchemes.set(style, before);
      changedList.push({ kind: 'scheme', target: style, before: before, priority: priority });
      style.setProperty('color-scheme', wanted, priority);
    }

    function rules(sheet) {
      var list;
      try {
        list = sheet.cssRules;
      } catch (error) {
        // A sheet from another site keeps its rules to itself.
        found.unreadable++;
        return;
      }
      if (!list) return;
      for (var index = 0; index < list.length; index++) {
        var rule = list[index];
        if (rule.media && typeof rule.media.mediaText === 'string') media(rule.media);
        if (rule.style) declarations(rule.style);
        // An imported sheet, and rules nested in "@supports", "@layer" or a style rule.
        if (rule.styleSheet) rules(rule.styleSheet);
        if (rule.cssRules && rule.cssRules.length) rules(rule);
      }
    }

    var sheets = [].slice.call(document.styleSheets || []);
    if (document.adoptedStyleSheets) sheets = sheets.concat([].slice.call(document.adoptedStyleSheets));
    sheets.forEach(function (sheet) {
      try {
        if (sheet.media && typeof sheet.media.mediaText === 'string') media(sheet.media);
      } catch (error) {
        // Counted below, when its rules are asked for.
      }
      rules(sheet);
    });

    // <meta name="color-scheme" content="light dark"> lets the browser choose for the whole page.
    [].slice.call(document.querySelectorAll('meta[name="color-scheme"]')).forEach(function (meta) {
      var before = meta.hasAttribute('data-ae-before')
        ? meta.getAttribute('data-ae-before')
        : meta.getAttribute('content');
      if (!choosesScheme(before)) return;
      found.schemes++;
      if (!schemeForced || meta.hasAttribute('data-ae-before')) return;
      meta.setAttribute('data-ae-before', before);
      changedList.push({ kind: 'meta', target: meta, before: before });
      meta.setAttribute('content', wanted);
    });
    return found;
  }

  var observer = null;
  var lastTheme = '';
  /** Looks at what the browser does by itself, does the rest, and tells the interface how it went. */
  function applyTheme() {
    if (!document.documentElement) return;
    // Our marks on <meta> are taken off first, so the page is judged by what it wrote itself.
    restoreStyles();
    [].slice.call(document.querySelectorAll('meta[data-ae-before]')).forEach(function (meta) {
      meta.removeAttribute('data-ae-before');
    });
    var media = nativeMedia();
    var scheme = nativeScheme();
    mediaForced = wanted !== null && media !== null && media !== wanted;
    schemeForced = wanted !== null && scheme !== null && scheme !== wanted;
    var found = applyStyles();
    for (var key in handedOut) handedOut[key].refresh();
    var report = {
      theme: wanted,
      // 'browser': the browser shows the theme in this frame by itself. 'rewritten': the page's styles were rewritten.
      how: wanted === null ? 'system' : mediaForced || schemeForced ? 'rewritten' : 'browser',
      media: found.media,
      schemes: found.schemes,
      scripts: pageAsked,
      unreadable: found.unreadable,
    };
    // What we changed ourselves is not news about the page.
    if (observer) observer.takeRecords();
    var text = JSON.stringify(report);
    if (text === lastTheme || !parentOrigin) return;
    lastTheme = text;
    say('theme', report);
  }

  function setTheme(theme) {
    wanted = theme === 'light' || theme === 'dark' ? theme : null;
    if (ownsName) window.name = 'ae-preview:' + (wanted || 'system');
    applyTheme();
  }

  var themeTimer = null;
  function themeSoon() {
    if (wanted === null || themeTimer !== null) return;
    themeTimer = setTimeout(function () {
      themeTimer = null;
      applyTheme();
    }, 0);
  }

  /** Whether a change of the page can have brought new styles: a style sheet came, or its text or its media changed. */
  function bringsStyles(record) {
    var target = record.target;
    if (record.type === 'attributes') return /^(LINK|STYLE|META)$/.test(target.nodeName);
    if (record.type === 'characterData') return Boolean(target.parentNode && target.parentNode.nodeName === 'STYLE');
    if (target.nodeName === 'STYLE') return true;
    for (var index = 0; index < record.addedNodes.length; index++) {
      var node = record.addedNodes[index];
      if (node.nodeType !== 1 || node === overlayHost) continue;
      if (/^(LINK|STYLE|META)$/.test(node.nodeName)) return true;
      if (node.querySelector && node.querySelector('style, link, meta[name="color-scheme"]')) return true;
    }
    return false;
  }

  // Style sheets come and go: the page is still loading, or a hot update swapped one.
  if (typeof MutationObserver === 'function') {
    observer = new MutationObserver(function (records) {
      if (records.some(bringsStyles)) themeSoon();
    });
    observer.observe(document, {
      childList: true,
      subtree: true,
      characterData: true,
      attributes: true,
      attributeFilter: ['media', 'href', 'rel', 'disabled'],
    });
  }
  // A sheet that was still on its way is here now.
  document.addEventListener(
    'load',
    function (event) {
      if (event.target && event.target.nodeName === 'LINK') themeSoon();
    },
    true,
  );
  // Rules added by a script do not show up as a change of the page.
  if (window.CSSStyleSheet && CSSStyleSheet.prototype.insertRule) {
    var nativeInsert = CSSStyleSheet.prototype.insertRule;
    CSSStyleSheet.prototype.insertRule = function () {
      var result = nativeInsert.apply(this, arguments);
      themeSoon();
      return result;
    };
  }
  // The system's own theme changed under us: what the browser does by itself may be different now.
  if (nativeMatchMedia) {
    var systemTheme = nativeMatchMedia('(prefers-color-scheme: dark)');
    var onSystem = function () {
      if (wanted !== null) applyTheme();
    };
    if (systemTheme.addEventListener) systemTheme.addEventListener('change', onSystem);
    else if (systemTheme.addListener) systemTheme.addListener(onSystem);
  }

  // The frame is named after the theme it should show, so the theme is known before the first
  // script of the page runs and asks: many pages ask once, at their start. The name is kept
  // in step with the theme, so the next page loaded into this frame starts right too. (A name
  // set while a page is leaving does not last in every engine; one set earlier does.)
  var named = /^ae-preview:(light|dark|system)$/.exec(window.name || '');
  /** Whether the frame's name is ours to keep. A page that names its window itself keeps that name. */
  var ownsName = Boolean(named);
  if (named) {
    wanted = named[1] === 'system' ? null : named[1];
    if (wanted) applyTheme();
  }

  // ---------- where an element comes from ----------

  /** "src/Login.tsx:42:7" and the like: a file, a line and perhaps a column, read from the right. */
  function splitPlace(text) {
    var parts = String(text).split(':');
    var numbers = [];
    while (parts.length > 1 && numbers.length < 2 && /^\d+$/.test(parts[parts.length - 1])) {
      numbers.unshift(Number(parts.pop()));
    }
    if (!numbers.length || !parts.join(':')) return null;
    return { file: parts.join(':'), line: numbers[0], column: numbers.length > 1 ? numbers[1] : null };
  }

  /** What build plugins write on elements in development, and how each is read. */
  var ATTRIBUTE_TRACES = [
    // react-dev-inspector
    function (el) {
      var file = el.getAttribute('data-inspector-relative-path');
      var line = Number(el.getAttribute('data-inspector-line'));
      if (!file || !line) return null;
      return { file: file, line: line, column: Number(el.getAttribute('data-inspector-column')) || null };
    },
    // Astro
    function (el) {
      var file = el.getAttribute('data-astro-source-file');
      var place = String(el.getAttribute('data-astro-source-loc') || '').split(':');
      if (!file || !Number(place[0])) return null;
      return { file: file, line: Number(place[0]), column: Number(place[1]) || null };
    },
    // A file in one attribute and a line in another.
    function (el) {
      var file = el.getAttribute('data-source-file');
      var line = Number(el.getAttribute('data-source-line'));
      if (!file || !line) return null;
      return { file: file, line: line, column: Number(el.getAttribute('data-source-column')) || null };
    },
    // code-inspector-plugin: "file:line:column:tag"
    function (el) {
      var value = el.getAttribute('data-insp-path');
      return value ? splitPlace(value.replace(/:[^:\d][^:]*$/, '')) : null;
    },
    // "file:line:column" in one attribute: Vue's inspector, Solid's locator, Qwik, jsx-loc and plain "data-source".
    function (el) {
      var names = ['data-v-inspector', 'data-source-loc', 'data-qwik-inspector', 'data-loc', 'data-source'];
      for (var index = 0; index < names.length; index++) {
        var value = el.getAttribute(names[index]);
        var place = value ? splitPlace(value) : null;
        if (place) return place;
      }
      return null;
    },
    // Sentry's annotations name the file and the component, without a line.
    function (el) {
      var file = el.getAttribute('data-sentry-source-file');
      if (!file) return null;
      return { file: file, line: null, column: null, component: el.getAttribute('data-sentry-component') };
    },
  ];

  function attributeTrace(el) {
    for (var index = 0; index < ATTRIBUTE_TRACES.length; index++) {
      var found = ATTRIBUTE_TRACES[index](el);
      if (found) return found;
    }
    return null;
  }

  function reactFiber(el) {
    for (var key in el) {
      if (key.indexOf('__reactFiber$') === 0 || key.indexOf('__reactInternalInstance$') === 0) return el[key];
    }
    return null;
  }

  /** The name a React component goes by, through the wrappers `memo` and `forwardRef` put around it. */
  function reactName(type) {
    if (!type) return null;
    if (typeof type === 'function') return type.displayName || type.name || null;
    if (typeof type === 'object') {
      return type.displayName || reactName(type.render) || reactName(type.type) || null;
    }
    return null;
  }

  function reactTrace(el) {
    var fiber = reactFiber(el);
    if (!fiber) return null;
    var names = [];
    for (var up = fiber.return; up && names.length < 3; up = up.return) {
      var name = reactName(up.type);
      if (name && names.indexOf(name) < 0) names.unshift(name);
    }
    // React up to 18 notes where each element was written, when the app is built for development.
    var source = fiber._debugSource;
    return {
      components: names,
      file: source && source.fileName ? source.fileName : null,
      line: source && source.lineNumber ? source.lineNumber : null,
      column: source && source.columnNumber ? source.columnNumber : null,
    };
  }

  function vueTrace(el) {
    var instance = el.__vueParentComponent;
    if (instance && instance.type) {
      var names = [];
      var file = null;
      for (var up = instance; up && names.length < 3; up = up.parent) {
        var type = up.type || {};
        var name =
          type.__name ||
          type.name ||
          (type.__file
            ? String(type.__file)
                .split(/[\\/]/)
                .pop()
                .replace(/\.\w+$/, '')
            : null);
        if (name) names.unshift(name);
        if (up === instance) file = type.__file || null;
      }
      return { components: names, file: file, line: null, column: null };
    }
    // Vue 2 keeps the component on its root element.
    var vm = el.__vue__;
    if (vm && vm.$options) {
      return {
        components: vm.$options.name ? [vm.$options.name] : [],
        file: vm.$options.__file || null,
        line: null,
        column: null,
      };
    }
    return null;
  }

  function svelteTrace(el) {
    var meta = el.__svelte_meta;
    var loc = meta && meta.loc;
    if (!loc || !loc.file) return null;
    // Svelte 3 and 4 count lines from 0 and say so by also giving `char`; Svelte 5 counts
    // them from 1. Both count columns from 0. (Read off what their compilers write.)
    var fromZero = typeof loc.char === 'number';
    var name = String(loc.file)
      .split(/[\\/]/)
      .pop()
      .replace(/\.svelte$/, '');
    return {
      components: name ? [name] : [],
      file: loc.file,
      line: typeof loc.line === 'number' ? loc.line + (fromZero ? 1 : 0) : null,
      column: typeof loc.column === 'number' ? loc.column + 1 : null,
    };
  }

  /**
   * Where an element was written, as far as the page says. Nothing is made up: a framework
   * that leaves no trace gives no file, and a file found on an element further out is marked as such.
   */
  function traceOf(el) {
    var trace = { components: [], file: null, line: null, column: null, from: null, nearby: false };
    var steps = 0;
    for (var node = el; node && node.nodeType === 1 && steps < 25; node = node.parentElement, steps++) {
      var own = node === el;
      var framework = reactTrace(node) || vueTrace(node) || svelteTrace(node);
      if (framework && !trace.components.length) trace.components = framework.components || [];
      var written = attributeTrace(node);
      var found = written || (framework && framework.file ? framework : null);
      if (found) {
        trace.file = found.file;
        trace.line = found.line || null;
        trace.column = found.column || null;
        trace.from = written ? 'attribute' : reactFiber(node) ? 'react' : node.__svelte_meta ? 'svelte' : 'vue';
        // Vue names the file of the component an element belongs to, which is its place however deep the element is.
        trace.nearby = !own && trace.from !== 'vue';
        if (written && written.component && !trace.components.length) trace.components = [written.component];
        break;
      }
    }
    return trace;
  }

  // ---------- describing an element ----------

  function cssEscape(text) {
    if (window.CSS && CSS.escape) return CSS.escape(text);
    return String(text).replace(/[^a-zA-Z0-9_-]/g, function (c) {
      return '\\' + c;
    });
  }

  function isOnly(selector) {
    try {
      return document.querySelectorAll(selector).length === 1;
    } catch (error) {
      return false;
    }
  }

  /** A selector that finds this element again, in this frame and in the others. */
  function selectorOf(el) {
    if (el === document.documentElement) return 'html';
    if (el === document.body) return 'body';
    var parts = [];
    for (var node = el; node && node.nodeType === 1; node = node.parentElement) {
      if (node === document.body) {
        parts.unshift('body');
        break;
      }
      if (node === document.documentElement) {
        parts.unshift('html');
        break;
      }
      if (node.id && isOnly('#' + cssEscape(node.id))) {
        parts.unshift('#' + cssEscape(node.id));
        break;
      }
      var testId = node.getAttribute('data-testid');
      if (testId && isOnly('[data-testid="' + cssEscape(testId) + '"]')) {
        parts.unshift('[data-testid="' + cssEscape(testId) + '"]');
        break;
      }
      var part = node.tagName.toLowerCase();
      var parent = node.parentElement;
      if (parent) {
        var twins = [].filter.call(parent.children, function (child) {
          return child.tagName === node.tagName;
        });
        if (twins.length > 1) part += ':nth-of-type(' + (twins.indexOf(node) + 1) + ')';
      }
      parts.unshift(part);
    }
    return parts.join(' > ');
  }

  function shortText(el) {
    var text = el.getAttribute('aria-label') || el.getAttribute('alt') || '';
    if (!text && /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName)) text = el.getAttribute('placeholder') || '';
    if (!text) text = el.innerText || el.textContent || '';
    text = String(text).replace(/\s+/g, ' ').trim();
    return text.length > 80 ? text.slice(0, 79) + '…' : text;
  }

  function describe(el) {
    var rect = el.getBoundingClientRect();
    var trace = traceOf(el);
    var classes = typeof el.className === 'string' ? el.className.split(/\s+/).filter(Boolean).slice(0, 3) : [];
    return {
      tag: el.tagName.toLowerCase(),
      id: el.id || null,
      classes: classes,
      role: el.getAttribute('role'),
      text: shortText(el),
      width: Math.round(rect.width),
      height: Math.round(rect.height),
      disabled: el.disabled === true || el.getAttribute('aria-disabled') === 'true',
      selector: selectorOf(el),
      components: trace.components,
      file: trace.file,
      line: trace.line,
      column: trace.column,
      from: trace.from,
      nearby: trace.nearby,
    };
  }

  /** The short name drawn next to an element: its component when known, else its tag. */
  function nameOf(about) {
    var name = about.components.length
      ? about.components[about.components.length - 1]
      : about.tag + (about.id ? '#' + about.id : about.classes.length ? '.' + about.classes[0] : '');
    if (about.file && !about.nearby) {
      name += ' · ' + String(about.file).split(/[\\/]/).pop() + (about.line ? ':' + about.line : '');
    }
    return name;
  }

  // ---------- what is drawn over the page ----------

  var overlayHost = null;
  var overlay = null;
  var parts = {};

  function px(value) {
    return Math.round(value) + 'px';
  }

  /** Our own layer over the page, apart from the page's styles. Made the first time it is needed. */
  function ensureOverlay() {
    if (overlayHost && overlayHost.isConnected) return;
    overlayHost = document.createElement('div');
    overlayHost.setAttribute('data-ae-preview', '');
    overlayHost.style.cssText =
      'all: initial; position: fixed; left: 0; top: 0; width: 100%; height: 100%; z-index: 2147483647; pointer-events: none;';
    overlay = overlayHost.attachShadow ? overlayHost.attachShadow({ mode: 'open' }) : overlayHost;
    // Styles are set through the element, not written as attributes: a page with a strict
    // content policy forbids the second and allows the first.
    var fill = 'position: fixed; left: 0; top: 0; width: 100%; height: 100%;';
    var box = 'position: fixed; display: none; box-sizing: border-box; pointer-events: none;';
    var label =
      'position: fixed; display: none; padding: 2px 8px; border-radius: 4px; color: #fff; white-space: nowrap; ' +
      'font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; max-width: 90vw; overflow: hidden; ' +
      'text-overflow: ellipsis; pointer-events: none;';
    function make(id, css, tag) {
      var el = tag ? document.createElementNS(SVG, tag) : document.createElement('div');
      el.setAttribute('id', id);
      el.style.cssText = css;
      parts[id] = el;
      return el;
    }
    overlay.appendChild(make('catch', fill + ' display: none; pointer-events: auto;'));
    overlay
      .appendChild(make('ink', fill + ' overflow: visible; pointer-events: none;', 'svg'))
      .appendChild(make('strokes', '', 'g'));
    overlay.appendChild(make('badges', 'position: fixed; left: 0; top: 0; pointer-events: none;'));
    overlay.appendChild(make('mark', box));
    overlay.appendChild(make('mark-label', label + ' background: ' + BLUE + ';'));
    overlay.appendChild(make('hover', box + ' outline: 2px solid ' + BLUE + '; background: rgba(31, 111, 235, 0.10);'));
    overlay.appendChild(make('hover-label', label + ' background: #0B0D10;'));
    parts['catch'].addEventListener('mousemove', onCatchMove);
    parts['catch'].addEventListener('mouseleave', clearHover);
    parts['catch'].addEventListener('click', onCatchClick);
    parts['catch'].addEventListener('pointerdown', onDrawStart);
    parts['catch'].addEventListener('pointermove', onDrawMove);
    parts['catch'].addEventListener('pointerup', onDrawEnd);
    parts['catch'].addEventListener('pointercancel', onDrawEnd);
    document.documentElement.appendChild(overlayHost);
    showTool();
  }

  /** A page that draws its whole document anew can drop our layer with the rest. It is put back. */
  function keepOverlay() {
    if (!overlayHost || overlayHost.isConnected || !document.documentElement) return;
    if (tool === 'none' && !markSpec && !ink.length) return;
    document.documentElement.appendChild(overlayHost);
  }

  /** Puts a box of ours exactly over an element, and a label under it (above, when there is no room under). */
  function place(box, label, el) {
    var rect = el.getBoundingClientRect();
    box.style.display = 'block';
    box.style.left = px(rect.left);
    box.style.top = px(rect.top);
    box.style.width = px(rect.width);
    box.style.height = px(rect.height);
    if (!label) return;
    label.style.display = 'block';
    var height = label.offsetHeight || 22;
    var under = rect.bottom + 8;
    var top = under + height > window.innerHeight ? Math.max(0, rect.top - height - 8) : under;
    label.style.left = px(Math.max(0, Math.min(rect.left, window.innerWidth - label.offsetWidth)));
    label.style.top = px(top);
  }

  /** The element of the page at a point of the window, looking through our own layer. */
  function elementAt(x, y) {
    var stack = document.elementsFromPoint ? document.elementsFromPoint(x, y) : [document.elementFromPoint(x, y)];
    for (var index = 0; index < stack.length; index++) {
      if (stack[index] && stack[index] !== overlayHost) return stack[index];
    }
    return null;
  }

  // ---------- picking an element ----------

  var tool = 'none';
  var hovered = null;
  /** The element this frame shows as picked, and whether it was picked here or in another frame. */
  var marked = null;
  var markSpec = null;

  function setTool(next) {
    next = next === 'pick' || next === 'draw' ? next : 'none';
    if (next === tool) return;
    tool = next;
    if (tool !== 'none') ensureOverlay();
    showTool();
    clearHover();
  }

  /** Lays the layer that catches the pointer over the page while a tool is on, and takes it off after. */
  function showTool() {
    if (!overlayHost) return;
    parts['catch'].style.display = tool === 'none' ? 'none' : 'block';
    parts['catch'].style.cursor = 'crosshair';
    // A finger scrolls the page while picking, and draws while drawing.
    parts['catch'].style.touchAction = tool === 'draw' ? 'none' : 'auto';
  }

  function clearHover() {
    hovered = null;
    if (!overlayHost) return;
    parts.hover.style.display = 'none';
    parts['hover-label'].style.display = 'none';
  }

  function onCatchMove(event) {
    if (tool !== 'pick') return;
    var el = elementAt(event.clientX, event.clientY);
    if (!el || el === hovered) return;
    hovered = el;
    var about = describe(el);
    parts['hover-label'].textContent = nameOf(about) + '  ' + about.width + ' × ' + about.height;
    place(parts.hover, parts['hover-label'], el);
  }

  function onCatchClick(event) {
    if (tool !== 'pick') return;
    event.preventDefault();
    event.stopPropagation();
    var el = elementAt(event.clientX, event.clientY);
    if (el) pick(el);
  }

  function pick(el) {
    clearHover();
    say('picked', { element: describe(el) });
  }

  function pickParent() {
    if (marked && marked.parentElement && marked !== document.documentElement) pick(marked.parentElement);
  }

  /** Shows the picked element: `mark` is `{ selector, own, label }`, or null to show none. */
  function setMark(mark) {
    markSpec = mark || null;
    marked = null;
    if (!markSpec) {
      if (overlayHost) {
        parts.mark.style.display = 'none';
        parts['mark-label'].style.display = 'none';
      }
      return;
    }
    ensureOverlay();
    drawMark(true);
  }

  var lastFound = null;
  function drawMark(tell) {
    if (!markSpec || !overlayHost) return;
    var el = null;
    try {
      el = document.querySelector(markSpec.selector);
    } catch (error) {
      el = null;
    }
    if (el !== marked || tell) {
      marked = el;
      var found = Boolean(el);
      if (tell || found !== lastFound) say('marked', { selector: markSpec.selector, found: found });
      lastFound = found;
    }
    if (!el) {
      parts.mark.style.display = 'none';
      parts['mark-label'].style.display = 'none';
      return;
    }
    // Picked here: a solid line. The same element on another width: a dashed one.
    parts.mark.style.outline = '3px ' + (markSpec.own ? 'solid ' : 'dashed ') + BLUE;
    parts.mark.style.outlineOffset = '3px';
    parts['mark-label'].textContent = markSpec.own ? markSpec.label || nameOf(describe(el)) : 'the same element here';
    place(parts.mark, parts['mark-label'], el);
  }

  // The page scrolls, grows and changes under the marks; they follow.
  window.addEventListener(
    'scroll',
    function () {
      drawMark(false);
      placeInk();
      clearHover();
    },
    true,
  );
  window.addEventListener('resize', function () {
    drawMark(false);
    placeInk();
  });
  setInterval(function () {
    keepOverlay();
    drawMark(false);
  }, 400);

  window.addEventListener(
    'keydown',
    function (event) {
      if (event.key === 'Escape' && tool !== 'none') say('escape', {});
    },
    true,
  );

  // ---------- drawing on the page ----------

  /** The marks drawn in this frame: `{ n, d }`, with `d` a path in the page's own coordinates. */
  var ink = [];
  var drawing = null;
  var SVG = 'http://www.w3.org/2000/svg';

  function scrollLeft() {
    return window.pageXOffset || document.documentElement.scrollLeft || 0;
  }
  function scrollTop() {
    return window.pageYOffset || document.documentElement.scrollTop || 0;
  }

  /** Keeps the drawings on the part of the page they were drawn on while the page scrolls. */
  function placeInk() {
    if (!overlayHost) return;
    parts.strokes.setAttribute('transform', 'translate(' + -scrollLeft() + ' ' + -scrollTop() + ')');
    parts.badges.style.transform = 'translate(' + -scrollLeft() + 'px, ' + -scrollTop() + 'px)';
  }

  function boxOf(points) {
    var box = { left: Infinity, top: Infinity, right: -Infinity, bottom: -Infinity };
    points.forEach(function (point) {
      box.left = Math.min(box.left, point[0]);
      box.top = Math.min(box.top, point[1]);
      box.right = Math.max(box.right, point[0]);
      box.bottom = Math.max(box.bottom, point[1]);
    });
    return box;
  }

  function pathOf(points) {
    return points
      .map(function (point, index) {
        return (index ? 'L' : 'M') + Math.round(point[0]) + ' ' + Math.round(point[1]);
      })
      .join(' ');
  }

  function pointsOf(d) {
    var numbers = String(d).match(/-?\d+(\.\d+)?/g) || [];
    var points = [];
    for (var index = 0; index + 1 < numbers.length; index += 2)
      points.push([Number(numbers[index]), Number(numbers[index + 1])]);
    return points;
  }

  function strokeElement(d) {
    var path = document.createElementNS(SVG, 'path');
    path.setAttribute('d', d);
    path.setAttribute('fill', 'none');
    path.setAttribute('stroke', RED);
    path.setAttribute('stroke-width', '3');
    path.setAttribute('stroke-linecap', 'round');
    path.setAttribute('stroke-linejoin', 'round');
    return path;
  }

  /** Shows exactly these drawings. The interface keeps them, so they survive a reload of the page. */
  function setInk(next) {
    ink = next.filter(function (stroke) {
      return stroke && typeof stroke.d === 'string';
    });
    if (!ink.length && !overlayHost) return;
    ensureOverlay();
    parts.strokes.textContent = '';
    parts.badges.textContent = '';
    ink.forEach(function (stroke) {
      parts.strokes.appendChild(strokeElement(stroke.d));
      var box = boxOf(pointsOf(stroke.d));
      var badge = document.createElement('span');
      badge.textContent = String(stroke.n);
      badge.style.cssText =
        'position: absolute; width: 22px; height: 22px; border-radius: 50%; background: ' +
        RED +
        '; color: #fff; ' +
        'font: 700 12px/22px system-ui, sans-serif; text-align: center; left: ' +
        px(box.right - 8) +
        '; top: ' +
        px(box.top - 14) +
        ';';
      parts.badges.appendChild(badge);
    });
    placeInk();
  }

  function onDrawStart(event) {
    if (tool !== 'draw' || event.button > 0) return;
    event.preventDefault();
    var point = [event.clientX + scrollLeft(), event.clientY + scrollTop()];
    drawing = { points: [point], path: strokeElement(pathOf([point])) };
    parts.strokes.appendChild(drawing.path);
    placeInk();
    try {
      parts['catch'].setPointerCapture(event.pointerId);
    } catch (error) {
      // Without capture a stroke simply ends at the edge of the frame.
    }
  }

  function onDrawMove(event) {
    if (!drawing) return;
    drawing.points.push([event.clientX + scrollLeft(), event.clientY + scrollTop()]);
    drawing.path.setAttribute('d', pathOf(drawing.points));
  }

  function onDrawEnd() {
    if (!drawing) return;
    var done = drawing;
    drawing = null;
    var box = boxOf(done.points);
    // A click without a stroke is not a drawing.
    if (box.right - box.left < 6 && box.bottom - box.top < 6) {
      if (done.path.parentNode) done.path.parentNode.removeChild(done.path);
      return;
    }
    // The interface numbers the drawing and sends back the whole set, which redraws them all.
    if (done.path.parentNode) done.path.parentNode.removeChild(done.path);
    say('drawn', {
      d: pathOf(done.points),
      box: {
        left: Math.round(box.left),
        top: Math.round(box.top),
        width: Math.round(box.right - box.left),
        height: Math.round(box.bottom - box.top),
      },
      element: describeAround(box),
    });
  }

  /**
   * The element a drawing is around: the one under its middle, widened to the largest
   * element that still fits inside the drawing (with a little room, for a loose circle).
   */
  function describeAround(box) {
    var x = (box.left + box.right) / 2 - scrollLeft();
    var y = (box.top + box.bottom) / 2 - scrollTop();
    var el = elementAt(
      Math.max(0, Math.min(x, window.innerWidth - 1)),
      Math.max(0, Math.min(y, window.innerHeight - 1)),
    );
    if (!el) return null;
    var roomX = Math.max(12, (box.right - box.left) * 0.15);
    var roomY = Math.max(12, (box.bottom - box.top) * 0.15);
    function fits(node) {
      var rect = node.getBoundingClientRect();
      return (
        rect.left + scrollLeft() >= box.left - roomX &&
        rect.right + scrollLeft() <= box.right + roomX &&
        rect.top + scrollTop() >= box.top - roomY &&
        rect.bottom + scrollTop() <= box.bottom + roomY
      );
    }
    while (el.parentElement && el.parentElement !== document.documentElement && fits(el.parentElement))
      el = el.parentElement;
    return describe(el);
  }

  // The first word: only that the script is here. Whoever frames the page learns nothing more
  // until it has shown itself to be the interface.
  window.parent.postMessage({ source: FROM_PAGE, type: 'ready' }, '*');
})(typeof AE_PREVIEW_CONFIG === 'object' && AE_PREVIEW_CONFIG ? AE_PREVIEW_CONFIG : { parents: [] });
