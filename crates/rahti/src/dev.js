// Rahti's dev client. Injected into every page, and served, only while dev
// mode is on — a release build never sees this file.
(() => {
  const logUrl = "/__rahti/dev/log";
  const queue = [];
  let flushTimer = null;

  const limit = (value, max = 12000) => {
    const text = String(value);
    return text.length <= max
      ? text
      : `${text.slice(0, max)}…[truncated]`;
  };

  const describe = (value) => {
    if (value instanceof Error) {
      return limit(`${value.name}: ${value.message}\n${value.stack || ""}`);
    }
    if (typeof value === "string") return limit(value);
    if (typeof value === "bigint") return `${value}n`;
    if (typeof value === "function") return `[Function ${value.name || "anonymous"}]`;
    if (value === undefined) return "undefined";

    const seen = new WeakSet();
    try {
      return limit(JSON.stringify(value, (_key, child) => {
        if (typeof child === "bigint") return `${child}n`;
        if (child instanceof Error) {
          return {
            name: child.name,
            message: child.message,
            stack: child.stack,
          };
        }
        if (child && typeof child === "object") {
          if (seen.has(child)) return "[Circular]";
          seen.add(child);
        }
        return child;
      }));
    } catch {
      return limit(Object.prototype.toString.call(value));
    }
  };

  const component = () => {
    const active = document.activeElement;
    const root = active?.closest?.("[pp-component]");
    if (!root) return "";
    const name = root.getAttribute("pp-component") || "";
    const element = [
      root.tagName.toLowerCase(),
      root.id ? `#${root.id}` : "",
    ].join("");
    return name ? `${name} (${element})` : element;
  };

  const flush = () => {
    flushTimer = null;
    if (queue.length === 0) return;
    const batch = queue.splice(0, 50);
    const body = JSON.stringify(batch);

    // Deliberately silent on failure: reporting a failed report through
    // console.warn/error would enqueue itself forever.
    fetch(logUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body,
      keepalive: true,
      credentials: "same-origin",
    }).catch(() => {});
  };

  const enqueue = (entry) => {
    queue.push({
      timestamp: new Date().toISOString(),
      url: location.href,
      component: component(),
      ...entry,
    });
    if (queue.length >= 50) {
      flush();
    } else if (flushTimer === null) {
      flushTimer = setTimeout(flush, 25);
    }
  };

  for (const level of ["warn", "error"]) {
    const original = console[level].bind(console);
    console[level] = (...args) => {
      original(...args);
      const trace = new Error(`console.${level}`).stack || "";
      enqueue({
        level,
        message: args.map(describe).join(" "),
        stack: trace,
      });
    };
  }

  addEventListener("error", (event) => {
    if (!(event instanceof ErrorEvent)) {
      const target = event.target;
      enqueue({
        level: "resource-error",
        message: `Failed to load ${target?.tagName?.toLowerCase?.() || "resource"}`,
        source: target?.src || target?.href || "",
      });
      return;
    }
    enqueue({
      level: "uncaught",
      message: event.message || describe(event.error),
      stack: event.error?.stack || "",
      source: event.filename || "",
      line: event.lineno || 0,
      column: event.colno || 0,
    });
  }, true);

  addEventListener("unhandledrejection", (event) => {
    enqueue({
      level: "unhandledrejection",
      message: describe(event.reason),
      stack: event.reason?.stack || "",
    });
  });

  addEventListener("pagehide", flush);

  const source = new EventSource("/__rahti/dev");
  let boot = null;

  source.addEventListener("boot", (event) => {
    // First message of the session: remember which server answered.
    if (boot === null) {
      boot = event.data;
      return;
    }
    // EventSource reconnected on its own and the id moved, so the process
    // was rebuilt and restarted underneath us and this page is stale. This
    // is what makes a `page.rs` edit reach the browser: the server does not
    // have to survive the restart, only be recognisably different after it.
    if (boot !== event.data) location.reload();
  });

  source.addEventListener("reload", () => location.reload());

  // Stylesheets can be swapped in place. Re-pointing `href` re-fetches the
  // sheet without rebuilding the DOM, so scroll position and any client
  // state survive an edit.
  source.addEventListener("css", () => {
    for (const link of document.querySelectorAll('link[rel="stylesheet"]')) {
      const url = new URL(link.href, location.href);
      url.searchParams.set("rahti-dev", Date.now().toString());
      link.href = url.href;
    }
  });
})();
