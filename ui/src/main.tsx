import '@fontsource-variable/inter';
import '@fontsource-variable/jetbrains-mono';
import '@fontsource-variable/space-grotesk';
import './styles/app.css';

import { render } from 'solid-js/web';
import { App } from './app/App';
import { watchRequests } from './core/requests';
import { startStore } from './core/store';

startStore();
watchRequests();
render(() => <App />, document.getElementById('root')!);
