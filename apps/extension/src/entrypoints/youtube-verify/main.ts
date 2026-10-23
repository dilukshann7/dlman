import { browser } from 'wxt/browser';
import './style.css';

const token = location.hash.slice(1);
const status = document.querySelector<HTMLParagraphElement>('#status')!;
const video = document.querySelector<HTMLParagraphElement>('#video')!;
const share = document.querySelector<HTMLButtonElement>('#share')!;
const cancel = document.querySelector<HTMLButtonElement>('#cancel')!;

async function send(type: string) {
  const response = await browser.runtime.sendMessage({ type, token }) as { success?: boolean; error?: string; url?: string };
  if (!response?.success) throw new Error(response?.error || 'Verification is unavailable. Start again in DLMan.');
  return response;
}

void send('youtube-verification-details').then((response) => {
  video.textContent = response.url || '';
  status.textContent = 'Keep the YouTube tab open until sharing finishes.';
  share.disabled = false;
}).catch((error: Error) => { status.textContent = error.message; });

share.addEventListener('click', () => {
  share.disabled = true;
  cancel.disabled = true;
  status.textContent = 'Sharing session with DLMan…';
  void send('youtube-verification-approve').then(() => {
    status.textContent = 'Session shared. Return to DLMan; video formats will reload automatically. You can close this window.';
    cancel.textContent = 'Close';
    cancel.disabled = false;
    cancel.onclick = () => window.close();
  }).catch((error: Error) => {
    status.textContent = error.message;
    share.disabled = false;
    cancel.disabled = false;
  });
});

cancel.addEventListener('click', () => {
  if (cancel.textContent === 'Close') return;
  cancel.disabled = true;
  void send('youtube-verification-cancel').then(() => window.close()).catch((error: Error) => {
    status.textContent = error.message;
    cancel.disabled = false;
  });
});
