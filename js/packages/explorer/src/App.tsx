import { Routes, Route } from "react-router-dom";
import { Layout } from "./components/Layout";
import { ConnectionProvider } from "./contexts/ConnectionContext";
import { AddressView } from "./pages/AddressView";
import { Dashboard } from "./pages/Dashboard";
import { BlockList } from "./pages/BlockList";
import { BlockDetail } from "./pages/BlockDetail";
import { TransactionDetail } from "./pages/TransactionDetail";

export default function App() {
  return (
    <ConnectionProvider>
      <Layout>
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/blocks" element={<BlockList />} />
          <Route path="/block/:hashOrHeight" element={<BlockDetail />} />
          <Route path="/tx/:txid" element={<TransactionDetail />} />
          <Route path="/address/:address" element={<AddressView />} />
        </Routes>
      </Layout>
    </ConnectionProvider>
  );
}
