import { Routes, Route } from "react-router-dom";
import { Layout } from "./components/Layout";
import { Dashboard } from "./pages/Dashboard";
import { BlockDetail } from "./pages/BlockDetail";
import { TransactionDetail } from "./pages/TransactionDetail";
import { NetworkPage } from "./pages/Network";

export default function App() {
  return (
    <Layout>
      <Routes>
        <Route path="/" element={<Dashboard />} />
        <Route path="/block/:hashOrHeight" element={<BlockDetail />} />
        <Route path="/tx/:txid" element={<TransactionDetail />} />
        <Route path="/network" element={<NetworkPage />} />
      </Routes>
    </Layout>
  );
}
