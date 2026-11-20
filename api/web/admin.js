const token = localStorage.getItem('lvl_admin_token');
const userList = document.querySelector('#user-list');
const userModal = document.querySelector('#user-modal');
const passwordModal = document.querySelector('#password-modal');
const userForm = document.querySelector('#user-form');
const passwordForm = document.querySelector('#password-form');
const toast = document.querySelector('#toast');
let users = [];

if (!token) window.location.replace('/login');

async function api(path, options = {}) {
  const response = await fetch(path, {
    ...options,
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}`, ...(options.headers || {}) }
  });
  if (response.status === 401 || response.status === 403) {
    localStorage.removeItem('lvl_admin_token');
    window.location.replace('/login');
    throw new Error('Sesión terminada');
  }
  const payload = await response.json().catch(() => null);
  if (!response.ok) throw new Error(payload?.error || 'La operación no pudo completarse');
  return payload;
}

function notify(text) {
  toast.textContent = text;
  toast.classList.remove('hidden');
  window.clearTimeout(notify.timer);
  notify.timer = window.setTimeout(() => toast.classList.add('hidden'), 3200);
}

function initials(user) {
  return (user.display_name || user.username).split(/\s+/).slice(0, 2).map((part) => part[0]).join('').toUpperCase();
}

function actionButton(label, handler, danger = false) {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = `button small ${danger ? 'danger' : 'secondary'}`;
  button.textContent = label;
  button.addEventListener('click', handler);
  return button;
}

function renderUsers(filter = '') {
  const query = filter.trim().toLowerCase();
  const visible = users.filter((user) => `${user.username} ${user.display_name}`.toLowerCase().includes(query));
  userList.replaceChildren();
  if (!visible.length) {
    const empty = document.createElement('div');
    empty.className = 'empty';
    empty.textContent = query ? 'No hay usuarios que coincidan.' : 'Todavía no hay usuarios.';
    userList.append(empty);
    return;
  }

  for (const user of visible) {
    const row = document.createElement('article');
    row.className = 'user-row';

    const primary = document.createElement('div');
    primary.className = 'user-primary';
    const avatar = document.createElement('span');
    avatar.className = 'avatar';
    avatar.textContent = initials(user);
    const names = document.createElement('div');
    const display = document.createElement('strong');
    display.textContent = user.display_name;
    const username = document.createElement('small');
    username.textContent = `@${user.username}${user.is_self ? ' · tú' : ''}`;
    names.append(display, username);
    primary.append(avatar, names);

    const role = document.createElement('div');
    role.className = 'user-cell'; role.dataset.label = 'Rol';
    role.textContent = user.is_admin ? 'Administrador' : 'Usuario';

    const state = document.createElement('div');
    state.className = 'user-cell'; state.dataset.label = 'Estado';
    const badge = document.createElement('span');
    badge.className = `badge ${user.status === 1 ? '' : 'off'}`;
    badge.textContent = user.status === 1 ? `● Activo · ${user.active_sessions}` : '● Suspendido';
    state.append(badge);

    const actions = document.createElement('div');
    actions.className = 'row-actions';
    if (!user.is_self && !user.is_root) {
      actions.append(actionButton(user.status === 1 ? 'Suspender' : 'Activar', () => toggleStatus(user)));
      actions.append(actionButton('Contraseña', () => openPassword(user)));
      actions.append(actionButton('Eliminar', () => removeUser(user), true));
    } else {
      const label = document.createElement('span');
      label.className = 'user-cell';
      label.textContent = user.is_root ? 'Cuenta raíz protegida' : 'Cuenta activa protegida';
      actions.append(label);
    }
    row.append(primary, role, state, actions);
    userList.append(row);
  }
}

async function loadDashboard() {
  try {
    const [overview, userPayload] = await Promise.all([api('/admin/api/overview'), api('/admin/api/users')]);
    document.querySelector('#metric-total').textContent = overview.total_users;
    document.querySelector('#metric-active').textContent = overview.active_users;
    document.querySelector('#metric-sessions').textContent = overview.active_sessions;
    document.querySelector('#metric-books').textContent = overview.address_books;
    document.querySelector('#welcome-copy').textContent = `Sesión de ${overview.current_user} · API v${overview.version}`;
    users = userPayload.users;
    renderUsers(document.querySelector('#search').value);
  } catch (error) {
    userList.innerHTML = `<div class="empty">${error.message}</div>`;
  }
}

async function toggleStatus(user) {
  try {
    await api(`/admin/api/users/${user.id}/status`, { method: 'PUT', body: JSON.stringify({ active: user.status !== 1 }) });
    notify(user.status === 1 ? 'Usuario suspendido' : 'Usuario activado');
    await loadDashboard();
  } catch (error) { notify(error.message); }
}

function openPassword(user) {
  passwordForm.reset();
  passwordForm.user_id.value = user.id;
  document.querySelector('#password-user').textContent = `Actualizar acceso de @${user.username}. Sus sesiones abiertas se cerrarán.`;
  passwordModal.classList.remove('hidden');
  passwordForm.password.focus();
}

async function removeUser(user) {
  if (!window.confirm(`¿Eliminar a @${user.username}? Esta acción también borra sus sesiones y libreta.`)) return;
  try {
    await api(`/admin/api/users/${user.id}`, { method: 'DELETE' });
    notify('Usuario eliminado');
    await loadDashboard();
  } catch (error) { notify(error.message); }
}

document.querySelector('#open-create').addEventListener('click', () => {
  userForm.reset();
  document.querySelector('#user-form-message').textContent = '';
  userModal.classList.remove('hidden');
  userForm.username.focus();
});
document.querySelectorAll('.close-modal').forEach((button) => button.addEventListener('click', () => userModal.classList.add('hidden')));
document.querySelectorAll('.close-password').forEach((button) => button.addEventListener('click', () => passwordModal.classList.add('hidden')));
document.querySelector('#search').addEventListener('input', (event) => renderUsers(event.target.value));

userForm.addEventListener('submit', async (event) => {
  event.preventDefault();
  const errorNode = document.querySelector('#user-form-message');
  errorNode.textContent = '';
  try {
    await api('/admin/api/users', {
      method: 'POST',
      body: JSON.stringify({ username: userForm.username.value.trim(), display_name: userForm.display_name.value.trim(), password: userForm.password.value, is_admin: userForm.is_admin.checked })
    });
    userModal.classList.add('hidden');
    notify('Cuenta creada correctamente');
    await loadDashboard();
  } catch (error) { errorNode.textContent = error.message; }
});

passwordForm.addEventListener('submit', async (event) => {
  event.preventDefault();
  const errorNode = document.querySelector('#password-message');
  errorNode.textContent = '';
  try {
    await api(`/admin/api/users/${passwordForm.user_id.value}/password`, { method: 'PUT', body: JSON.stringify({ password: passwordForm.password.value }) });
    passwordModal.classList.add('hidden');
    notify('Contraseña actualizada y sesiones cerradas');
    await loadDashboard();
  } catch (error) { errorNode.textContent = error.message; }
});

document.querySelector('#logout').addEventListener('click', async () => {
  try { await api('/api/logout', { method: 'POST', body: '{}' }); } catch (_) {}
  localStorage.removeItem('lvl_admin_token');
  window.location.replace('/login');
});

loadDashboard();
