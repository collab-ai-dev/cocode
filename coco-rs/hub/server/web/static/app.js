function normalizeLanguageName(language) {
  var normalized = (language || "").toLowerCase();
  var aliases = {
    sh: "bash",
    shell: "bash",
    zsh: "bash",
    powershell: "bash",
    ps1: "bash",
    patch: "diff",
    udiff: "diff",
    md: "markdown",
    mdown: "markdown",
    rs: "rust",
    ts: "typescript",
    tsx: "typescript",
    js: "javascript",
    jsx: "javascript",
    py: "python",
    yml: "yaml",
    jsonl: "json",
    golang: "go",
    text: "plaintext",
    txt: "plaintext",
  };
  return aliases[normalized] || normalized || "plaintext";
}

function normalizeCodeBlocks(root) {
  (root || document).querySelectorAll("pre code").forEach(function (code) {
    var languageClass = Array.from(code.classList).find(function (className) {
      return className.indexOf("language-") === 0 || className.indexOf("lang-") === 0;
    });
    var language = languageClass
      ? languageClass.replace(/^lang(uage)?-/, "")
      : "plaintext";
    code.className = "language-" + normalizeLanguageName(language);
  });
}

function renderMarkdown(root) {
  if (!window.marked || !window.DOMPurify) {
    return;
  }

  window.marked.setOptions({
    async: false,
    breaks: false,
    gfm: true,
  });

  (root || document)
    .querySelectorAll("[data-render-markdown]")
    .forEach(function (container) {
      var source = container.querySelector("template.render-source");
      if (!source) {
        return;
      }
      var markdown = source.content ? source.content.textContent : source.textContent;
      var rendered = window.marked.parse(markdown);
      container.innerHTML = window.DOMPurify.sanitize(rendered, {
        USE_PROFILES: { html: true },
      });
    });
}

function highlightCode(root) {
  if (window.Prism) {
    normalizeCodeBlocks(root || document);
    window.Prism.highlightAllUnder(root || document);
  }
}

function timelineRows() {
  return Array.from(document.querySelectorAll("#event-stream .event"));
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function clearSearchHighlights(root) {
  (root || document).querySelectorAll("mark.search-hit").forEach(function (mark) {
    var parent = mark.parentNode;
    if (!parent) {
      return;
    }
    parent.replaceChild(document.createTextNode(mark.textContent || ""), mark);
    parent.normalize();
  });
}

function shouldSkipSearchHighlight(node) {
  var element = node.parentElement;
  if (!element) {
    return true;
  }
  return Boolean(element.closest("script, style, template, mark.search-hit"));
}

function highlightSearchTerms(row, terms) {
  if (!terms.length) {
    return 0;
  }

  var pattern = terms
    .slice()
    .sort(function (left, right) {
      return right.length - left.length;
    })
    .map(escapeRegExp)
    .join("|");
  var expression = new RegExp(pattern, "gi");
  var walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT, {
    acceptNode: function (node) {
      if (shouldSkipSearchHighlight(node) || !node.nodeValue.trim()) {
        return NodeFilter.FILTER_REJECT;
      }
      expression.lastIndex = 0;
      return expression.test(node.nodeValue)
        ? NodeFilter.FILTER_ACCEPT
        : NodeFilter.FILTER_REJECT;
    },
  });
  var nodes = [];
  var current = walker.nextNode();
  var hitCount = 0;

  while (current) {
    nodes.push(current);
    current = walker.nextNode();
  }

  nodes.forEach(function (node) {
    var text = node.nodeValue;
    var fragment = document.createDocumentFragment();
    var lastIndex = 0;
    var match;

    expression.lastIndex = 0;
    while ((match = expression.exec(text)) !== null) {
      if (match.index > lastIndex) {
        fragment.appendChild(document.createTextNode(text.slice(lastIndex, match.index)));
      }
      var mark = document.createElement("mark");
      mark.className = "search-hit";
      mark.textContent = match[0];
      fragment.appendChild(mark);
      hitCount += 1;
      lastIndex = expression.lastIndex;
    }
    if (lastIndex < text.length) {
      fragment.appendChild(document.createTextNode(text.slice(lastIndex)));
    }
    node.parentNode.replaceChild(fragment, node);
  });

  return hitCount;
}

function applyLoadedSearch() {
  var input = document.getElementById("timeline-search");
  var status = document.getElementById("timeline-search-status");
  var rows = timelineRows();

  if (!input) {
    return;
  }

  var terms = input.value
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);
  var visible = 0;
  var hitCount = 0;

  rows.forEach(function (row) {
    var haystack = [
      row.dataset.msgType,
      row.dataset.lane,
      row.dataset.tool,
      row.dataset.search,
      row.dataset.searchText,
    ]
      .join(" ")
      .toLowerCase();
    var matches = terms.every(function (term) {
      return haystack.indexOf(term) !== -1;
    });
    clearSearchHighlights(row);
    row.hidden = !matches;
    if (matches) {
      visible += 1;
      hitCount += highlightSearchTerms(row, terms);
    }
  });

  if (status) {
    status.textContent =
      terms.length === 0 ? "" : visible + " / " + rows.length + " rows, " + hitCount + " hits";
  }
}

function setRawDetails(open) {
  timelineRows().forEach(function (row) {
    row.querySelectorAll("details.event__raw").forEach(function (details) {
      details.open = open;
    });
  });
}

function localDateTimeValue(date) {
  if (!(date instanceof Date) || Number.isNaN(date.getTime())) {
    return "";
  }
  var offsetMs = date.getTimezoneOffset() * 60000;
  return new Date(date.getTime() - offsetMs).toISOString().slice(0, 16);
}

function dateFromLocalInput(input) {
  if (!input || !input.value) {
    return null;
  }
  var date = new Date(input.value);
  return Number.isNaN(date.getTime()) ? null : date;
}

function syncTimeRangeHiddenFields() {
  var fromInput = document.getElementById("time-from-local");
  var toInput = document.getElementById("time-to-local");
  var fromHidden = document.getElementById("time-from-rfc3339");
  var toHidden = document.getElementById("time-to-rfc3339");
  var fromDate = dateFromLocalInput(fromInput);
  var toDate = dateFromLocalInput(toInput);

  if (fromHidden) {
    fromHidden.value = fromDate ? fromDate.toISOString() : "";
  }
  if (toHidden) {
    toHidden.value = toDate ? toDate.toISOString() : "";
  }
}

function initializeTimeRangeControls() {
  var range = document.querySelector(".time-window");
  var fromInput = document.getElementById("time-from-local");
  var toInput = document.getElementById("time-to-local");
  var fromHidden = document.getElementById("time-from-rfc3339");
  var toHidden = document.getElementById("time-to-rfc3339");

  if (!range || !fromInput || !toInput) {
    return;
  }

  var start = range.dataset.rangeStart ? new Date(range.dataset.rangeStart) : null;
  var end = range.dataset.rangeEnd ? new Date(range.dataset.rangeEnd) : null;
  var min = localDateTimeValue(start);
  var max = localDateTimeValue(end);

  if (min) {
    fromInput.min = min;
    toInput.min = min;
  }
  if (max) {
    fromInput.max = max;
    toInput.max = max;
  }
  if (fromHidden && fromHidden.value) {
    fromInput.value = localDateTimeValue(new Date(fromHidden.value));
  }
  if (toHidden && toHidden.value) {
    toInput.value = localDateTimeValue(new Date(toHidden.value));
  }
}

document.addEventListener("htmx:afterSwap", function (event) {
  var root = event.detail && event.detail.target ? event.detail.target : document;
  renderMarkdown(root);
  highlightCode(root);
  applyLoadedSearch();
});

document.addEventListener("DOMContentLoaded", function () {
  renderMarkdown(document);
  highlightCode(document);
  initializeTimeRangeControls();
  applyLoadedSearch();
});

document.addEventListener("input", function (event) {
  if (event.target && event.target.id === "timeline-search") {
    applyLoadedSearch();
  } else if (
    event.target &&
    (event.target.id === "time-from-local" || event.target.id === "time-to-local")
  ) {
    syncTimeRangeHiddenFields();
  }
});

document.addEventListener("submit", function (event) {
  if (event.target && event.target.id === "timeline-filter-form") {
    syncTimeRangeHiddenFields();
  }
});

document.addEventListener("htmx:configRequest", function (event) {
  if (event.target && event.target.id === "timeline-filter-form") {
    syncTimeRangeHiddenFields();
  }
});

document.addEventListener("click", function (event) {
  var timeButton = event.target.closest("[data-time-action]");
  if (timeButton && timeButton.dataset.timeAction === "full") {
    var fromInput = document.getElementById("time-from-local");
    var toInput = document.getElementById("time-to-local");
    if (fromInput) {
      fromInput.value = "";
    }
    if (toInput) {
      toInput.value = "";
    }
    syncTimeRangeHiddenFields();
    var form = document.getElementById("timeline-filter-form");
    if (form) {
      form.requestSubmit();
    }
    return;
  }

  var button = event.target.closest("[data-detail-action]");
  if (!button) {
    return;
  }
  setRawDetails(button.dataset.detailAction === "expand");
});
