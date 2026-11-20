const form = document.querySelector('#login-form');
const message = document.querySelector('#login-message');
const password = document.querySelector('#password');
const showPassword = document.querySelector('#show-password');

showPassword.addEventListener('click', () => {
  const reveal = password.type === 'password';
  password.type = reveal ? 'text' : 'password';
  showPassword.setAttribute('aria-label', reveal ? 'Ocultar contraseña' : 'Mostrar contraseña');
});

form.addEventListener('submit', async (event) => {
  event.preventDefault();
  message.textContent = '';
  const button = form.querySelector('button[type="submit"]');
  button.disabled = true;
  button.textContent = 'Verificando…';

  try {
    const response = await fetch('/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        username: form.username.value.trim(),
        password: form.password.value,
        deviceInfo: { platform: 'web-admin' }
      })
    });
    const payload = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(payload.error || 'No fue posible iniciar sesión');
    if (!payload.user?.is_admin) throw new Error('Esta cuenta no tiene permisos administrativos');
    localStorage.setItem('lvl_admin_token', payload.access_token);
    window.location.replace('/admin');
  } catch (error) {
    message.textContent = error.message;
    button.disabled = false;
    button.textContent = 'Ingresar →';
  }
});

const existingToken = localStorage.getItem('lvl_admin_token');
if (existingToken) {
  fetch('/admin/api/overview', { headers: { Authorization: `Bearer ${existingToken}` } })
    .then((response) => { if (response.ok) window.location.replace('/admin'); })
    .catch(() => {});
}
