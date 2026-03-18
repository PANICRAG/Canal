-- Admin user seeding migration
-- Note: The actual admin user is created by the application on startup
-- This migration ensures the auth columns exist and are properly configured

-- Ensure pgcrypto extension is available for password hashing
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- Add index on email for faster lookups
CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);

-- Add index on status for filtering active users
CREATE INDEX IF NOT EXISTS idx_users_status ON users(status);

-- Add index on role for admin queries
CREATE INDEX IF NOT EXISTS idx_users_role ON users(role);

-- Create a function to check if admin exists (used by application)
CREATE OR REPLACE FUNCTION admin_exists()
RETURNS BOOLEAN AS $$
BEGIN
    RETURN EXISTS(SELECT 1 FROM users WHERE role = 'admin' AND status = 'active');
END;
$$ LANGUAGE plpgsql;
