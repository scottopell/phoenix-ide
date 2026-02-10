import { useState } from 'react';

interface AuthBannerProps {
  oauthUrl: string;
  deviceCode: string;
}

export function AuthBanner({ oauthUrl, deviceCode }: AuthBannerProps) {
  const [copied, setCopied] = useState(false);

  const copyCode = async () => {
    try {
      // Try modern clipboard API first
      if (navigator.clipboard && navigator.clipboard.writeText) {
        await navigator.clipboard.writeText(deviceCode);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      } else {
        // Fallback for non-HTTPS contexts
        const textArea = document.createElement('textarea');
        textArea.value = deviceCode;
        textArea.style.position = 'fixed';
        textArea.style.left = '-999999px';
        document.body.appendChild(textArea);
        textArea.select();
        try {
          document.execCommand('copy');
          setCopied(true);
          setTimeout(() => setCopied(false), 2000);
        } catch (err) {
          console.error('Copy failed:', err);
          alert('Copy failed. Please manually copy: ' + deviceCode);
        }
        document.body.removeChild(textArea);
      }
    } catch (err) {
      console.error('Copy failed:', err);
      alert('Copy failed. Please manually copy: ' + deviceCode);
    }
  };

  return (
    <div className="auth-banner">
      <div className="auth-banner-icon">🔐</div>
      <div className="auth-banner-content">
        <div className="auth-banner-title">AI Gateway Authentication Required</div>
        <div className="auth-banner-steps">
          <div className="auth-step">
            1.{' '}
            <a href={oauthUrl} target="_blank" rel="noopener noreferrer">
              Open authentication page
            </a>
          </div>
          <div className="auth-step">
            2. Enter code:
            <code className="device-code">{deviceCode}</code>
            <button onClick={copyCode} className="copy-btn">
              {copied ? '✓' : 'Copy'}
            </button>
          </div>
        </div>
        <div className="auth-banner-status">Waiting for authentication...</div>
      </div>
    </div>
  );
}
