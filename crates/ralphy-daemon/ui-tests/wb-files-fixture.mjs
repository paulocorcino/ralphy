// The pages and the peer shell that more than one wb-files test file builds.
import { loadComponent, loadShell } from "./harness.mjs";

// One page with the shell and the files, and a window that delivers the
// events it is sent, as the browser does. `init()` adds the listeners, as
// Alpine does when it builds the component.
export function page() {
  const listeners = {};
  const events = [];
  const loaded = loadShell({
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => {
        events.push(e);
        for (const fn of listeners[e.type] || []) fn(e);
        return true;
      },
    },
  });
  const files = loadComponent("wbFiles", { from: loaded, magics: { $nextTick: (fn) => fn(), $refs: {} } });
  files.scope.init();
  const fire = (type, detail) => loaded.window.dispatchEvent({ type, detail });
  return { shell: loaded.state, files: files.scope, window: loaded.window, fire, events, listeners };
}

export const FILES_PEER = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
export const FILES_REF = `${FILES_PEER}/o/r`;

// The files with the peer project open and the peer in the given fleet state.
// `shell` is the Alpine store `projects`: it holds the fleet, which the files
// read through `fleetGroups`.
export function peerFilesShell(state) {
  const loaded = loadShell();
  const files = loadComponent("wbFiles", { from: loaded });
  const s = files.scope;
  s.$store.projects.setProjects([]);
  loaded.state.$store.projects.fleetPeers = [{ daemon_id: FILES_PEER, name: "corcino-mac", environment: "macOS 15", tunnel: true, state, diagnosis: `diagnosis of ${state}` }];
  s.$store.projects.setOpen(FILES_REF);
  s.treeMem();
  const { window, document } = loaded;
  return { state: s, data: files.data, shell: loaded.state.$store.projects, window, document };
}

// The options `mountTree` gives Wunderbaum, from a stand-in that keeps them.
export function mountedTreeOptions(s, window, document) {
  let options = null;
  // No tree channel: this checks the tree's own options.
  window.WBDaemon = {};
  const realMar10 = globalThis.mar10;
  globalThis.mar10 = {
    Wunderbaum: function (o) {
      options = o;
    },
  };
  const host = { addEventListener() {} };
  document.querySelector = (sel) => (sel === ".files-pane .wb-host" ? host : null);
  s.$store.projects.setProjects([{ key: FILES_REF, slug: "o/r", daemon: FILES_PEER, tree: [] }]);
  s.useDaemonTree = () => true;
  s.loadTreeLevel = () => Promise.resolve([]);
  try {
    s.mountTree();
  } finally {
    globalThis.mar10 = realMar10;
  }
  return options;
}
