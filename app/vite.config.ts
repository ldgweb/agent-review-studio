import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  // tauri:dev waits on exactly this address: fail loudly if the port is
  // taken instead of silently moving to 5174
  server: { port: 5173, strictPort: true },
});
