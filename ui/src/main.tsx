import '@fontsource-variable/inter';
import '@fontsource-variable/jetbrains-mono';
import '@fontsource-variable/space-grotesk';
import './styles/app.css';

import { render } from 'solid-js/web';
import { App } from './app/App';
import { watchNotifications } from './core/notify';
import { watchRequests } from './core/requests';
import { watchSettings } from './core/settings';
import { startStore } from './core/store';

startStore();
watchRequests();
watchSettings();
watchNotifications();
render(() => <App />, document.getElementById('root')!);
