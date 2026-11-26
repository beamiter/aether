export function installWorkflows(document, {context, retained, invoke, insert, returnToTerminal, announce}) {
  const toggle = document.querySelector('#workflows');
  const picker = document.querySelector('#workflow-picker');
  const search = document.querySelector('#workflow-search');
  const results = document.querySelector('#workflow-results');
  const fields = document.querySelector('#workflow-fields');
  const args = document.querySelector('#workflow-args');
  const status = document.querySelector('#workflow-status');
  const insertButton = document.querySelector('#workflow-insert');
  let active = null;
  let openChain = Promise.resolve();
  let suppressEnterRepeat = false;

  function close(restoreFocus = false) {
    active = null;
    results.dataset.searchReady = 'false';
    picker.hidden = true;
    toggle.setAttribute('aria-expanded', 'false');
    toggle.disabled = !context();
    if (restoreFocus) returnToTerminal();
  }

  function isCurrent(view) {
    const next = context();
    return active === view && (
      next?.owner === view.owner && next.prompt === view.prompt || view.selected && retained(view.owner)
    );
  }

  function updateOwner() {
    if (active && !isCurrent(active)) close();
    toggle.disabled = !active && !context();
    if(active?.selected)insertButton.textContent = context()?.owner?.session === active.owner.session
      ? 'Insert at cursor' : `Insert in shell ${active.owner.epoch}`;
  }

  function resultsReady(view) {
    return Boolean(view?.catalogId && !view.searching && view.renderedQuery === search.value);
  }

  function list(view, response, query) {
    view.catalogId = response.catalogId;
    view.renderedQuery = query;
    results.replaceChildren(...response.rows.map(row => {
      const item = document.createElement('li');
      const select = document.createElement('button');
      select.type = 'button';
      select.textContent = row.name;
      select.addEventListener('click', () => selectWorkflow(view, row.name));
      item.append(select);
      if (row.description) {
        const description = document.createElement('span');
        description.textContent = row.description;
        item.append(description);
      }
      return item;
    }));
    status.textContent = response.empty
      ? 'No workflows found. Add a TOML or YAML workflow to the Aether configuration directory.'
      : response.rows.length ? '' : 'No matching workflows.';
  }

  function togglePicker() {
    if (active) {
      close(true);
      return;
    }
    const current = context();
    if (!current) return;
    const view = {...current, query: '', catalogId: null, selected: null, selecting: false, inserting: false};
    active = view;
    search.value = '';
    search.parentElement.hidden = results.hidden = false;
    fields.hidden = true;
    insertButton.disabled = false;
    insertButton.textContent = 'Insert at cursor';
    results.replaceChildren();
    results.dataset.searchReady = 'false';
    picker.hidden = false;
    toggle.setAttribute('aria-expanded', 'true');
    status.textContent = 'Loading workflows…';
    search.focus();
    // A cancelled scan may finish late. Issue a newer open after it returns
    // so its catalog cannot replace the one displayed in the newer picker.
    openChain = openChain.then(async () => {
      if (!isCurrent(view)) return;
      try {
        const response = await invoke({
          action: 'open', owner: view.owner, prompt: view.prompt,
        });
        if (!isCurrent(view)) return;
        list(view, response, '');
        await searchLibrary(view);
      } catch (error) {
        if (isCurrent(view)) status.textContent = String(error?.message ?? error);
      }
    });
  }

  async function searchLibrary(view) {
    if (!view?.catalogId || view.searching || view.selected) return;
    view.searching = true;
    results.dataset.searchReady = 'false';
    try {
      while (isCurrent(view) && !view.selected && view.query !== search.value) {
        view.query = search.value;
        const response = await invoke({action: 'search', catalogId: view.catalogId, query: view.query});
        if (isCurrent(view) && !view.selected && view.query === search.value) list(view, response, view.query);
      }
    } catch (error) {
      if (isCurrent(view) && !view.selected) {
        view.query = view.renderedQuery;
        status.textContent = String(error?.message ?? error);
      }
    } finally {
      view.searching = false;
      if (isCurrent(view)) results.dataset.searchReady = String(resultsReady(view));
    }
  }

  search.addEventListener('input', () => searchLibrary(active));

  async function selectWorkflow(view, name) {
    if (!isCurrent(view) || view.selecting || view.selected || !resultsReady(view)) return;
    const query = view.query;
    view.selecting = true;
    try {
      const response = await invoke({action: 'select', catalogId: view.catalogId, name});
      if (!isCurrent(view) || search.value !== query) return;
      view.selected = response.name;
      search.parentElement.hidden = results.hidden = true;
      fields.hidden = false;
      document.querySelector('#workflow-title').textContent = response.name;
      document.querySelector('#workflow-command').textContent = response.command;
      const inputs = [];
      args.replaceChildren(...response.args.map(arg => {
        const label = document.createElement('label');
        const value = document.createElement('input');
        value.type = 'text';
        value.spellcheck = false;
        value.autocomplete = 'off';
        value.maxLength = 65536;
        value.value = arg.default ?? '';
        value.title = arg.description;
        label.append(arg.name, value);
        inputs.push([arg.name, value]);
        return label;
      }));
      view.inputs = inputs;
      status.textContent = '';
      (inputs[0]?.[1] ?? insertButton).focus();
    } catch (error) {
      if (isCurrent(view)) status.textContent = String(error?.message ?? error);
    } finally {
      view.selecting = false;
    }
  }

  insertButton.addEventListener('click', async () => {
    const view = active;
    if (!view?.selected || !isCurrent(view) || view.inserting) return;
    const restoreFocus = picker.contains(document.activeElement);
    view.inserting = insertButton.disabled = true;
    status.textContent = 'Inserting at the shell cursor…';
    let confirmed = false;
    try {
      await insert({
        catalogId: view.catalogId,
        name: view.selected,
        values: Object.fromEntries(view.inputs.map(([name, value]) => [name, value.value])),
      }, view.owner, reason => {
        confirmed = true;
        if (!isCurrent(view)) return;
        if (reason) {
          status.textContent = reason;
          return;
        }
        suppressEnterRepeat = true;
        close(picker.contains(document.activeElement) || (restoreFocus && document.activeElement === document.body));
        announce('Workflow queued at the shell cursor. Review the line before pressing Enter.');
      });
    } catch (error) {
      confirmed = true;
      if(isCurrent(view))status.textContent = String(error?.message??error);
    } finally {
      view.inserting = false;
      if (active === view) insertButton.disabled = false;
      if (!confirmed && isCurrent(view)) {
        status.textContent = 'Insertion could not be confirmed. Check the live shell before trying again.';
      }
    }
  });

  toggle.addEventListener('click', togglePicker);
  document.querySelector('#workflow-close').addEventListener('click', () => close(true));
  document.querySelector('#workflow-back').addEventListener('click', () => {
    if (!active || active.inserting) return;
    active.selected = null;
    fields.hidden = true;
    search.parentElement.hidden = results.hidden = false;
    status.textContent = '';
    search.focus();
  });
  picker.addEventListener('keydown', event => {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      close(true);
    }
  });

  return {
    updateOwner,
    toggle: togglePicker,
    consumeEnterRepeat(event) {
      if (event.key !== 'Enter') return false;
      if (event.repeat) return suppressEnterRepeat;
      suppressEnterRepeat = false;
      return false;
    },
  };
}
