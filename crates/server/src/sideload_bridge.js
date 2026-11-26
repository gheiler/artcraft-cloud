(function () {
  var origin = window.__ARTCRAFT_PARENT__;
  var app = window.__ARTCRAFT_APP__;
  if (!origin || !app || window.__artcraftBridge) return;
  window.__artcraftBridge = true;
  var pending = {};
  var seq = 0;
  var store = {};
  window.__ARTCRAFT_TOKEN__ = null;
  function applySettings(settings) {
    if (!settings || typeof settings !== "object") return;
    for (var key in settings) {
      if (Object.prototype.hasOwnProperty.call(settings, key)) {
        store[key] = String(settings[key]);
      }
    }
  }
  if (window.__ARTCRAFT_SHIM_STORAGE__) {
  try {
    var memory = {
      getItem: function (key) {
        return Object.prototype.hasOwnProperty.call(store, key) ? store[key] : null;
      },
      setItem: function (key, value) {
        store[key] = String(value);
        parent.postMessage({ type: app + ":settings-set", key: String(key), value: String(value) }, origin);
      },
      removeItem: function (key) {
        delete store[key];
        parent.postMessage({ type: app + ":settings-remove", key: String(key) }, origin);
      },
      clear: function () {
        store = {};
      },
      key: function (index) {
        return Object.keys(store)[index] || null;
      },
    };
    Object.defineProperty(memory, "length", {
      get: function () {
        return Object.keys(store).length;
      },
    });
    Object.defineProperty(window, "localStorage", { configurable: true, get: function () { return memory; } });
  } catch (err) {}
  }
  window.addEventListener("message", function (ev) {
    if (ev.origin !== origin) return;
    var data = ev.data || {};
    if (data.type === app + ":host" || data.type === app + ":settings") {
      if (data.token) window.__ARTCRAFT_TOKEN__ = data.token;
      if (data.settings) applySettings(data.settings);
    }
    var wait = pending[data.id];
    if (!wait) return;
    if (data.type === app + ":picked") {
      delete pending[data.id];
      wait.resolve(data.files || []);
    } else if (data.type === app + ":pick-cancel") {
      delete pending[data.id];
      wait.reject(new DOMException("The user aborted a request.", "AbortError"));
    }
  });
  function ask(kind, extra) {
    var id = ++seq;
    return new Promise(function (resolve, reject) {
      pending[id] = { resolve: resolve, reject: reject };
      var msg = { type: app + ":" + kind, id: id };
      if (extra) {
        for (var key in extra) msg[key] = extra[key];
      }
      parent.postMessage(msg, origin);
    });
  }
  function asFiles(list) {
    return (list || []).map(function (file) {
      return new File([file.bytes], file.name, { type: file.type || "" });
    });
  }
  function acceptOf(options) {
    var types = (options && options.types) || [];
    return types
      .map(function (item) {
        return Object.values(item.accept || {}).flat().join(",");
      })
      .join(",");
  }
  window.showOpenFilePicker = function (options) {
      return ask("pick", {
        accept: acceptOf(options),
        multiple: !!(options && options.multiple),
      }).then(function (list) {
        return asFiles(list).map(function (file) {
          return {
            kind: "file",
            name: file.name,
            getFile: function () {
              return Promise.resolve(file);
            },
          };
        });
      });
  };
  window.showSaveFilePicker = function (options) {
      var name = (options && options.suggestedName) || "document";
      return Promise.resolve({
        name: name,
        kind: "file",
        createWritable: function () {
          var chunks = [];
          return Promise.resolve({
            write: function (data) {
              chunks.push(data);
              return Promise.resolve();
            },
            close: function () {
              return new Blob(chunks).arrayBuffer().then(function (buf) {
                parent.postMessage(
                  { type: app + ":save", name: name, saveAs: true, bytes: buf },
                  origin
                );
              });
            },
          });
        },
      });
  };
  var proto = HTMLInputElement.prototype;
  var click = proto.click;
  proto.click = function () {
    if (this.type === "file") {
      var input = this;
      ask("pick", { accept: this.accept || "", multiple: !!this.multiple })
        .then(function (list) {
          var transfer = new DataTransfer();
          asFiles(list).forEach(function (file) {
            transfer.items.add(file);
          });
          input.files = transfer.files;
          input.dispatchEvent(new Event("input", { bubbles: true }));
          input.dispatchEvent(new Event("change", { bubbles: true }));
        })
        .catch(function () {
          input.dispatchEvent(new Event("cancel"));
        });
      return;
    }
    return click.apply(this, arguments);
  };
})();
